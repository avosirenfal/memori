//! Turns a [`Report`] into text. Each section becomes data first (tables, a bar, warnings),
//! and only [`render`] turns that into lines, so `--no-headers` and `--no-help` apply
//! uniformly.

mod describe;
mod style;
pub mod table;

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt::Display;

use anstyle::Style;

use crate::analysis::breakdown::{Category, Slice};
use crate::analysis::{
    Adjustment, Cache, Fragmentation, Group, Kernel, Members, NOTICEABLE, Overview, Programs,
    Report, Slabs, StandsFor, Swap, Unexplained,
};
use crate::collections::sorted_by_key;
use crate::sections::SectionKind;
use crate::sources::Source;
use crate::sources::procs::Rollup;
use crate::sources::zfs::ArcStats;
use crate::units::Bytes;
use style::{BOLD, DIM, WARN};
use table::{Cell, Column, Row, Show, Table, paint};

/// What to print, and how.
#[derive(Debug, Clone)]
pub struct View {
    /// In report order.
    pub sections: Vec<SectionKind>,
    /// Break the overview's groups down into categories.
    pub breakdown: bool,
    pub show: Show,
}

pub fn render(report: &Report, view: &View) -> String {
    let title = view.show.headers.then(|| title(report));
    let sections = view
        .sections
        .iter()
        .filter_map(|&kind| section(kind, report, view.breakdown))
        .map(|section| section.render(view.show));
    let text: Vec<String> = title.into_iter().chain(sections).collect();
    format!("{}\n", text.join("\n\n"))
}

fn section(kind: SectionKind, report: &Report, breakdown: bool) -> Option<Section> {
    let blocks = match kind {
        SectionKind::Overview => overview(&report.overview, breakdown),
        SectionKind::Programs => programs(&report.programs),
        SectionKind::Zfs => from_source(report.zfs.as_ref(), |arc| zfs(arc))?,
        SectionKind::Kernel => kernel(&report.kernel),
        SectionKind::Cache => cache(&report.cache),
        SectionKind::Swap => swap(report.swap.as_ref()?),
        SectionKind::Fragmentation => from_source(report.fragmentation.as_ref(), fragmentation)?,
        SectionKind::Unexplained => unexplained(report.unexplained.as_ref()?),
    };
    Some(Section { kind, blocks })
}

/// A section's blocks, a warning if its source couldn't be read, or nothing if the
/// system doesn't have it.
fn from_source<T>(source: Source<T>, blocks: impl FnOnce(T) -> Vec<Block>) -> Option<Vec<Block>> {
    match source {
        Source::Available(value) => Some(blocks(value)),
        Source::Absent => None,
        problem => Some(warning(&problem).into_iter().collect()),
    }
}

/// Why a source couldn't be used.
fn warning<T>(source: &Source<T>) -> Option<Block> {
    match source {
        Source::Available(_) | Source::Absent => None,
        Source::NeedsRoot => Some(Block::Warning("needs root; run with sudo".to_owned())),
        Source::Broken(reason) => Some(Block::Warning(format!("couldn't read {reason}"))),
    }
}

// ─── Building blocks ─────────────────────────────────────────────────────────────────────

const WIDTH: usize = 80;
const BAR: usize = 72;

struct Section {
    kind: SectionKind,
    blocks: Vec<Block>,
}

enum Block {
    Table(Table),
    /// A line drawn as-is, such as the overview bar.
    Line(String),
    /// Advice about the output, shown only with both headers and help on.
    Warning(String),
}

impl Section {
    fn render(&self, show: Show) -> String {
        let heading = show.headers.then(|| heading(self.kind.spec().title));
        let blocks = self.blocks.iter().map(|block| block.render(show).join("\n"));
        let parts: Vec<String> =
            heading.into_iter().chain(blocks).filter(|block| !block.is_empty()).collect();
        parts.join("\n\n")
    }
}

impl Block {
    fn render(&self, show: Show) -> Vec<String> {
        match self {
            Self::Table(table) => table.render(show),
            Self::Line(line) => vec![format!("  {line}")],
            Self::Warning(text) => (show.headers && show.help)
                .then(|| paint(WARN, format!("  ⚠ {text}")))
                .into_iter()
                .collect(),
        }
    }
}

fn heading(title: &str) -> String {
    let rule = "━".repeat(WIDTH.saturating_sub(title.chars().count() + 4));
    paint(BOLD, format!("━━ {title} {rule}"))
}

/// A row of a "label  amount  detail  help" table.
struct Entry {
    label: Cell,
    amount: Cell,
    detail: Cell,
    help: Cell,
}

fn entry(indent: usize, label: impl Display, amount: impl Display) -> Entry {
    Entry {
        label: Cell::new(format!("{:indent$}{label}", "")),
        amount: Cell::new(amount),
        detail: Cell::default(),
        help: Cell::default(),
    }
}

impl Entry {
    fn detail(self, detail: impl Display) -> Self {
        Self { detail: Cell::new(detail), ..self }
    }

    fn help(self, help: &str) -> Self {
        Self { help: Cell::new(help), ..self }
    }

    fn bold(self) -> Self {
        Self { label: self.label.restyled(BOLD), amount: self.amount.restyled(BOLD), ..self }
    }

    fn into_row(self) -> Row {
        Row::Cells(vec![self.label, self.amount, self.detail, self.help])
    }
}

const ENTRY_COLUMNS: [Column; 4] = [Column::LEFT, Column::RIGHT, Column::LEFT, Column::HELP];

fn entries(entries: impl IntoIterator<Item = Entry>) -> Block {
    let rows = entries.into_iter().map(Entry::into_row).collect();
    Block::Table(Table::new(ENTRY_COLUMNS.to_vec(), rows))
}

/// Splits `width` cells among `weights` proportionally, using largest remainders so the
/// cells always add up to exactly `width`.
fn allocate_cells(weights: &[u64], width: usize) -> Vec<usize> {
    let total: u128 = weights.iter().map(|&w| u128::from(w)).sum();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let exact: Vec<(usize, u128)> = weights
        .iter()
        .map(|&w| {
            let scaled = u128::from(w) * width as u128;
            ((scaled / total) as usize, scaled % total)
        })
        .collect();
    let short = width - exact.iter().map(|(cells, _)| cells).sum::<usize>();
    let by_remainder = sorted_by_key((0..exact.len()).collect(), |&i| Reverse(exact[i].1));
    let bonus: HashSet<usize> = by_remainder.into_iter().take(short).collect();
    exact
        .iter()
        .enumerate()
        .map(|(i, (cells, _))| cells + usize::from(bonus.contains(&i)))
        .collect()
}

/// `weights` as a bar of `width` cells, each part drawn with its own glyph and style.
fn bar(parts: &[(u64, &str, Style)], width: usize) -> String {
    let weights: Vec<u64> = parts.iter().map(|(weight, _, _)| *weight).collect();
    parts
        .iter()
        .zip(allocate_cells(&weights, width))
        .filter(|(_, cells)| *cells > 0)
        .map(|((_, glyph, style), cells)| paint(*style, glyph.repeat(cells)))
        .collect()
}

// ─── Sections ────────────────────────────────────────────────────────────────────────────

fn title(report: &Report) -> String {
    let host = &report.host;
    let line = format!(
        "memori · {} · Linux {} · {} usable RAM",
        host.hostname.unwrap_or("?"),
        host.kernel.unwrap_or("?"),
        host.total
    );
    paint(BOLD, line)
}

fn overview(overview: &Overview, breakdown: bool) -> Vec<Block> {
    let total = overview.total;
    let slices: Vec<&Slice> =
        overview.groups.iter().flat_map(|group| group.slices.iter()).collect();
    let parts: Vec<(u64, &str, Style)> = slices
        .iter()
        .map(|slice| {
            (slice.bytes.0, style::glyph(slice.category.reclaim()), style::color(slice.category))
        })
        .collect();

    let share = |label: Cell, bytes: Bytes| {
        Row::Cells(vec![label, Cell::new(bytes), Cell::new(style::percent(bytes, total))])
    };
    let group_rows = overview.groups.iter().flat_map(|group: &Group| {
        // A group of one category is named after it, and needs no breakdown.
        let name = group
            .slices
            .only()
            .map_or(style::group_name(group.reclaim), |slice| style::label(slice.category));
        let marker = format!("{} ", style::glyph(group.reclaim));
        let header =
            share(Cell::spans([(marker.as_str(), Style::new()), (name, BOLD)]), group.bytes());
        let categories: Vec<Row> = if breakdown && group.slices.only().is_none() {
            group
                .slices
                .iter()
                .filter(|slice| slice.bytes >= NOTICEABLE)
                .map(|slice| {
                    let label = Cell::spans([
                        ("    ", Style::new()),
                        ("■", style::color(slice.category)),
                        (" ", Style::new()),
                        (style::label(slice.category), Style::new()),
                    ]);
                    share(label, slice.bytes)
                })
                .collect()
        } else {
            vec![]
        };
        std::iter::once(header).chain(categories)
    });
    let overcounted = overview
        .overcounted
        .map(|extra| entry(0, "Counted twice (overlapping counters)", extra).into_row());

    let adjustment = overview.adjustment.as_ref().map(|adjustment| match adjustment {
        Adjustment::HeldBack(bytes) => entry(0, "Held back by the kernel", format!("−{bytes}")),
        Adjustment::Extra(bytes) => entry(0, "Other reclaimable memory", format!("+{bytes}")),
    });
    let available = match (overview.arc_shrink, overview.total_available()) {
        (Some(shrink), Some(total)) => vec![
            entry(0, "Available", overview.available),
            entry(0, "ZFS ARC can shrink by", shrink),
            entry(0, "Total available", total).bold(),
        ],
        _ => vec![entry(0, "Available", overview.available).bold()],
    };
    let classic =
        breakdown.then(|| entry(0, "\"used\" as classic `free` reports it", overview.classic_used));
    let availability = adjustment.into_iter().chain(available).chain(classic).map(Entry::into_row);

    // One table, so amounts line up from the shares down to the total. Its third column
    // holds the shares, so unlike other entry tables it is right-aligned.
    let rows = group_rows.chain(overcounted).chain([Row::Gap]).chain(availability).collect();
    let columns = vec![Column::LEFT, Column::RIGHT, Column::RIGHT, Column::HELP];
    vec![Block::Line(bar(&parts, BAR)), Block::Table(Table::new(columns, rows))]
}

fn programs(programs: &Programs) -> Vec<Block> {
    // PID, Program, Procs, RAM, Swap, Command
    let cells = |pid: Option<u32>, name: String, stands_for: StandsFor, mem: Rollup| {
        let (processes, command) = match stands_for {
            StandsFor::Program { processes: Members::Many(count) } => (Cell::new(count), None),
            StandsFor::Program { processes: Members::One } => (Cell::default(), None),
            StandsFor::Process { command } => (Cell::default(), command),
        };
        let command =
            command.map_or_else(Cell::default, |c| Cell::styled(style::truncate(c, 48), DIM));
        let pid = pid.map_or_else(Cell::default, Cell::new);
        vec![pid, Cell::new(name), processes, Cell::new(mem.pss), Cell::new(mem.swap), command]
    };
    // Summary rows aren't one process, and list no process count.
    let summary = StandsFor::Program { processes: Members::One };
    let rows = programs.rows.iter().map(|row| {
        Row::Cells(cells(Some(row.pid), style::truncate(row.name, 32), row.stands_for, row.mem))
    });
    let hidden = programs.hidden.as_ref().map(|hidden| {
        Row::styled(cells(None, format!("…{} more", hidden.rows), summary, hidden.mem), DIM)
    });
    let total = &programs.total;
    let total =
        Row::Total(cells(None, format!("All {} processes", total.processes), summary, total.mem));
    let table = Table::new(
        vec![
            Column::right("PID"),
            Column::left("Program"),
            Column::right("Procs"),
            Column::right("RAM"),
            Column::right("Swap"),
            Column::left("Command"),
        ],
        rows.chain(hidden).chain([total]).collect(),
    );
    let unreadable = match programs.unreadable {
        0 => None,
        1 => Some("1 process of another user unreadable; run with sudo to include it".to_owned()),
        n => {
            Some(format!("{n} processes of other users unreadable; run with sudo to include them"))
        }
    };
    std::iter::once(Block::Table(table)).chain(unreadable.map(Block::Warning)).collect()
}

fn zfs(arc: &ArcStats) -> Vec<Block> {
    let s = &arc.sizes;
    let gauge = bar(
        &[
            (s.size.0, "█", style::color(Category::ZfsArc)),
            (s.max.saturating_sub(s.size).0, "░", Style::new()),
        ],
        30,
    );
    let rows = [
        Some(entry(0, "Size", s.size).detail(format!("of {} max", s.max))),
        Some(entry(0, "Target", s.target).help("size the ARC is steering toward")),
        Some(entry(0, "Minimum", s.min).help("it never shrinks below this")),
        Some(entry(0, "Can shrink by", arc.shrinkable())),
        Some(entry(2, "file data", s.data)),
        Some(entry(2, "metadata", s.metadata)),
        Some(
            entry(2, "bookkeeping", arc.bookkeeping())
                .help("headers, dnodes, dbufs; counted as slab"),
        ),
        arc.hit_ratio().map(|ratio| {
            entry(0, "Hit rate", format!("{:.1}%", 100.0 * ratio))
                .help("reads served from RAM since boot")
        }),
    ];
    vec![Block::Line(gauge), entries(rows.into_iter().flatten())]
}

fn kernel(kernel: &Kernel) -> Vec<Block> {
    let counters = entries([
        entry(0, style::label(Category::KernelSlab), kernel.slab_unreclaimable)
            .help("must stay in RAM"),
        entry(0, style::label(Category::KernelCaches), kernel.slab_reclaimable)
            .help("dropped under memory pressure"),
        entry(0, style::label(Category::PageTables), kernel.page_tables)
            .help("map programs' memory onto physical RAM"),
        entry(0, "Kernel stacks", kernel.stacks).help("one per thread"),
        entry(0, "Per-CPU data", kernel.percpu),
        entry(0, "vmalloc", kernel.vmalloc).help("large kernel buffers; overlaps the above"),
    ]);
    let slabs = match &kernel.slabs {
        Source::Available(slabs) => slab_blocks(slabs),
        problem => warning(problem).into_iter().collect(),
    };
    std::iter::once(counters).chain(slabs).collect()
}

fn slab_blocks(slabs: &Slabs) -> Vec<Block> {
    let rows = slabs.largest.iter().map(|cache| {
        Row::Cells(vec![
            Cell::new(&cache.name),
            Cell::new(cache.footprint),
            Cell::new(style::percent(cache.in_use(), cache.footprint)),
            Cell::new(cache.wasted()),
            Cell::new(style::freeable(cache.reclaimability)),
            Cell::new(describe::describe(&cache.name).unwrap_or_default()),
        ])
    });
    let total = Row::Total(vec![
        Cell::new(format!("All {} caches", slabs.count)),
        Cell::new(slabs.footprint),
        Cell::new(style::percent(slabs.in_use, slabs.footprint)),
        Cell::new(slabs.wasted),
    ]);
    let table = Table::new(
        vec![
            Column::left("Slab cache"),
            Column::right("Size"),
            Column::right("Full"),
            Column::right("Wasted"),
            Column::right("Freeable"),
            Column::help("What it holds"),
        ],
        rows.chain([total]).collect(),
    );
    let fragmentation = std::iter::once(
        entry(0, "Slab fragmentation", slabs.wasted)
            .detail(format!("{} of slab", style::percent(slabs.wasted, slabs.footprint)))
            .help("empty space inside partly filled slabs"),
    )
    .chain(slabs.wasted_unreclaimable.map(|wasted| {
        entry(2, "in unreclaimable caches", wasted)
            .help("stuck until neighbouring objects are freed")
    }));
    vec![Block::Table(table), entries(fragmentation)]
}

fn cache(cache: &Cache) -> Vec<Block> {
    let file_cache = entries([
        entry(0, style::label(Category::FileCache), cache.file)
            .help("copies of files read from disk"),
        entry(2, "recently used", cache.active).help("kept longest"),
        entry(2, "not recently used", cache.inactive).help("dropped first"),
        entry(2, "mapped by running programs", cache.mapped).help("code, libraries, mmapped files"),
        entry(2, "not yet written to disk", cache.unwritten)
            .help("written out before it can be dropped"),
    ]);
    let mounts =
        cache.mounts.iter().map(|mount| entry(2, style::truncate(&mount.path, 36), mount.used));
    let elsewhere = (cache.elsewhere >= NOTICEABLE)
        .then(|| entry(2, "elsewhere", cache.elsewhere).help("SysV/POSIX shm, memfd, GPU buffers"));
    let shared = entries(
        std::iter::once(
            entry(0, style::label(Category::SharedMemory), cache.shared)
                .help("can't be dropped; `free` calls it cache"),
        )
        .chain(mounts)
        .chain(elsewhere),
    );
    vec![file_cache, shared]
}

fn swap(swap: &Swap) -> Vec<Block> {
    let devices =
        swap.devices.available().into_iter().flat_map(|devices| devices.iter()).map(|device| {
            let label = format!(
                "{} ({})",
                style::truncate(&device.path, 30),
                style::swap_kind(&device.kind)
            );
            entry(2, label, device.used).detail(format!("of {}", device.size))
        });
    let zram = swap.zram.iter().map(|z| {
        let ratio = z.original.0 as f64 / z.ram_used.0.max(1) as f64;
        entry(2, format!("{} RAM used", z.name), z.ram_used)
            .detail(format!("holds {}, {ratio:.1}× compressed", z.original))
    });
    let rows =
        std::iter::once(entry(0, "Swap in use", swap.used).detail(format!("of {}", swap.total)))
            .chain(devices)
            .chain(zram)
            .chain([entry(0, "Swap cache", swap.cache).help("pages kept both in RAM and in swap")]);
    std::iter::once(entries(rows)).chain(warning(&swap.devices)).collect()
}

fn fragmentation(fragmentation: &Fragmentation) -> Vec<Block> {
    let total = fragmentation.total();
    let rows = fragmentation.free.iter().enumerate().map(|(index, (size, bytes))| {
        let row = entry(0, style::block_size(*size), *bytes).detail(style::percent(*bytes, total));
        // The verdict is about the largest blocks, which come first.
        if index == 0 { row.help(style::verdict(fragmentation.verdict)) } else { row }
    });
    vec![entries(rows)]
}

fn unexplained(unexplained: &Unexplained) -> Vec<Block> {
    let suspects = [
        (unexplained.vmalloc >= NOTICEABLE).then(|| {
            entry(2, "vmalloc (excluding kernel stacks)", unexplained.vmalloc)
                .help("possible source")
        }),
        (unexplained.balloon >= NOTICEABLE)
            .then(|| entry(2, "VM balloon", unexplained.balloon).help("possible source")),
        Some(
            entry(2, "pages drivers allocate directly", "?")
                .help("e.g. NVIDIA, i915; not reported"),
        ),
    ];
    let rows = std::iter::once(entry(0, "Not covered by any kernel counter", unexplained.amount))
        .chain(suspects.into_iter().flatten());
    vec![entries(rows)]
}

/// Removes terminal styling, for comparing rendered text in tests.
#[cfg(test)]
pub fn strip_ansi(text: &str) -> String {
    text.split('\u{1b}')
        .enumerate()
        .map(|(i, part)| match i {
            0 => part,
            _ => part.split_once('m').map_or(part, |(_, rest)| rest),
        })
        .collect()
}

#[cfg(test)]
mod tests;
