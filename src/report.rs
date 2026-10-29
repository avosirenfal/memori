//! Turns a [`Snapshot`] into the text report.
//!
//! Each section is built as data (tables and lines) and rendered afterwards, so that
//! `--no-headers` and `--no-help` apply uniformly.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt::Display;

use anstyle::{AnsiColor, Color, Style};

use crate::breakdown::{Breakdown, Category, Reclaim, Remainder, Slice};
use crate::buddy::{self, BlockSize};
use crate::procfs::sorted_by_key;
use crate::procs::{self, Rollup};
use crate::slab::SlabCache;
use crate::snapshot::Snapshot;
use crate::source::Source;
use crate::table::{Cell, Column, Row, Show, Table, paint};
use crate::units::Bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionKind {
    Overview,
    Programs,
    Zfs,
    Kernel,
    Cache,
    Swap,
    Fragmentation,
    Unexplained,
}

impl SectionKind {
    pub const ALL: [Self; 8] = [
        Self::Overview,
        Self::Programs,
        Self::Zfs,
        Self::Kernel,
        Self::Cache,
        Self::Swap,
        Self::Fragmentation,
        Self::Unexplained,
    ];
    pub const SUMMARY: [Self; 2] = [Self::Overview, Self::Programs];
}

pub struct Options {
    /// Sections to show, in report order.
    pub sections: Vec<SectionKind>,
    /// Break the overview's totals down into categories.
    pub detailed: bool,
    /// How many programs to list; `None` lists all of them.
    pub programs: Option<usize>,
    /// How many slab caches to list.
    pub slabs: usize,
    /// List individual processes instead of grouping them by program.
    pub per_process: bool,
    pub show: Show,
}

pub fn render(snapshot: &Snapshot, options: &Options) -> String {
    let breakdown = Breakdown::of(snapshot);
    let title = options.show.headers.then(|| title(snapshot));
    let sections = options
        .sections
        .iter()
        .filter_map(|kind| build(*kind, snapshot, &breakdown, options))
        .map(|section| section.render(options.show));
    let text: Vec<String> = title.into_iter().chain(sections).collect();
    format!("{}\n", text.join("\n\n"))
}

fn build(
    kind: SectionKind,
    snapshot: &Snapshot,
    breakdown: &Breakdown,
    options: &Options,
) -> Option<Section> {
    match kind {
        SectionKind::Overview => Some(overview(snapshot, breakdown, options.detailed)),
        SectionKind::Programs => Some(programs(snapshot, options)),
        SectionKind::Zfs => zfs(snapshot),
        SectionKind::Kernel => Some(kernel(snapshot, options.slabs)),
        SectionKind::Cache => Some(cache(snapshot, breakdown)),
        SectionKind::Swap => swap(snapshot),
        SectionKind::Fragmentation => fragmentation(snapshot),
        SectionKind::Unexplained => unexplained(snapshot, breakdown),
    }
}

// ─── Sections and blocks ─────────────────────────────────────────────────────────────────

struct Section {
    title: &'static str,
    blocks: Vec<Block>,
}

enum Block {
    Table(Table),
    /// A line drawn as-is, such as the overview bar.
    Line(String),
    Warning(String),
}

impl Section {
    fn new(title: &'static str, blocks: impl IntoIterator<Item = Block>) -> Self {
        Self { title, blocks: blocks.into_iter().collect() }
    }

    fn render(&self, show: Show) -> String {
        let heading = show.headers.then(|| heading(self.title));
        let blocks = self.blocks.iter().map(|block| block.render(show).join("\n"));
        let parts: Vec<String> =
            heading.into_iter().chain(blocks).filter(|b| !b.is_empty()).collect();
        parts.join("\n\n")
    }
}

impl Block {
    fn render(&self, show: Show) -> Vec<String> {
        match self {
            Self::Table(table) => table.render(show),
            Self::Line(line) => vec![format!("  {line}")],
            // Warnings are advice about the output, so they go with headers and help.
            Self::Warning(text) => (show.headers && show.help)
                .then(|| paint(WARN, format!("  ⚠ {text}")))
                .into_iter()
                .collect(),
        }
    }
}

/// A row of a "label  amount  detail  help" table.
struct Entry {
    label: Cell,
    amount: Cell,
    detail: Cell,
    help: Cell,
}

fn entry(indent: usize, label: &str, amount: impl Display) -> Entry {
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

fn entries(entries: impl IntoIterator<Item = Entry>) -> Block {
    let columns = vec![Column::left(""), Column::right(""), Column::left(""), Column::help("")];
    Block::Table(Table::new(columns, entries.into_iter().map(Entry::into_row).collect()))
}

// ─── Presentation ────────────────────────────────────────────────────────────────────────

const WIDTH: usize = 80;
const BAR: usize = 72;
/// Amounts smaller than this are left out of lists.
const NOTICEABLE: Bytes = Bytes::MIB;

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const WARN: Style = fg(AnsiColor::Yellow).bold();

const fn fg(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(color)))
}

fn heading(title: &str) -> String {
    let rule = "━".repeat(WIDTH.saturating_sub(title.chars().count() + 4));
    paint(BOLD, format!("━━ {title} {rule}"))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let kept: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

fn percent(part: Bytes, whole: Bytes) -> String {
    match 100.0 * part.fraction_of(whole) {
        p if p > 0.0 && p < 1.0 => "<1%".to_owned(),
        p => format!("{p:.0}%"),
    }
}

fn label(category: Category) -> &'static str {
    match category {
        Category::Programs => "Programs' own data (heap, stacks)",
        Category::SharedMemory => "Shared memory & tmpfs",
        Category::KernelSlab => "Kernel objects (unreclaimable slab)",
        Category::PageTables => "Page tables",
        Category::StacksAndPercpu => "Kernel stacks & per-CPU data",
        Category::CompressedSwap => "Compressed swap in RAM (zram/zswap)",
        Category::Gpu => "GPU buffers in system RAM (amdgpu GTT)",
        Category::HugePages => "Huge pages (reserved pool)",
        Category::Unexplained => "Unexplained",
        Category::ZfsArc => "ZFS ARC",
        Category::FileCache => "File cache",
        Category::KernelCaches => "Kernel caches (reclaimable slab)",
        Category::Free => "Free",
    }
}

fn color(category: Category) -> Style {
    fg(match category {
        Category::Programs => AnsiColor::Blue,
        Category::SharedMemory => AnsiColor::Cyan,
        Category::KernelSlab | Category::PageTables | Category::StacksAndPercpu => {
            AnsiColor::Magenta
        }
        Category::CompressedSwap => AnsiColor::BrightMagenta,
        Category::Gpu => AnsiColor::BrightBlue,
        Category::HugePages => AnsiColor::BrightCyan,
        Category::Unexplained => AnsiColor::Red,
        Category::ZfsArc => AnsiColor::Yellow,
        Category::FileCache => AnsiColor::Green,
        Category::KernelCaches => AnsiColor::BrightGreen,
        Category::Free => AnsiColor::BrightBlack,
    })
}

/// Each kind of reclaimability gets its own texture, so the bar reads without color too.
fn glyph(reclaim: Reclaim) -> &'static str {
    match reclaim {
        Reclaim::InUse => "█",
        Reclaim::Shrinkable => "▓",
        Reclaim::Cache => "▒",
        Reclaim::Free => "░",
    }
}

fn group_name(reclaim: Reclaim) -> &'static str {
    match reclaim {
        Reclaim::InUse => "In use",
        Reclaim::Shrinkable => "Shrinkable",
        Reclaim::Cache => "Cache",
        Reclaim::Free => "Free",
    }
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

// ─── Sections ────────────────────────────────────────────────────────────────────────────

fn title(snapshot: &Snapshot) -> String {
    let line = format!(
        "memori · {} · Linux {} · {} usable RAM",
        snapshot.hostname, snapshot.kernel, snapshot.meminfo.total
    );
    paint(BOLD, line)
}

fn overview(snapshot: &Snapshot, breakdown: &Breakdown, detailed: bool) -> Section {
    let mem = &snapshot.meminfo;
    let total = breakdown.total;
    let groups: Vec<(Reclaim, Vec<Slice>)> = Reclaim::ALL
        .iter()
        .map(|&reclaim| {
            let visible = breakdown.of_reclaim(reclaim).into_iter();
            (reclaim, visible.filter(|s| s.bytes >= NOTICEABLE).collect::<Vec<_>>())
        })
        .filter(|(_, slices)| !slices.is_empty())
        .collect();
    let group_sum = |reclaim: Reclaim| -> Bytes {
        groups.iter().filter(|(r, _)| *r == reclaim).flat_map(|(_, s)| s).map(|s| s.bytes).sum()
    };

    let ordered: Vec<Slice> = groups.iter().flat_map(|(_, slices)| slices.clone()).collect();
    let cells = allocate_cells(&ordered.iter().map(|s| s.bytes.0).collect::<Vec<_>>(), BAR);
    let bar: String = ordered
        .iter()
        .zip(cells)
        .filter(|(_, n)| *n > 0)
        .map(|(slice, n)| paint(color(slice.category), glyph(slice.category.reclaim()).repeat(n)))
        .collect();

    let group_rows = groups.iter().flat_map(|(reclaim, slices)| {
        // A group of one category is named after it, and needs no breakdown.
        let name = match slices[..] {
            [only] => label(only.category),
            _ => group_name(*reclaim),
        };
        let sum: Bytes = slices.iter().map(|s| s.bytes).sum();
        let marker = format!("{} ", glyph(*reclaim));
        let group = Row::Cells(vec![
            Cell::spans([(marker.as_str(), Style::new()), (name, BOLD)]),
            Cell::new(sum),
            Cell::new(percent(sum, total)),
        ]);
        let categories = (detailed && slices.len() > 1).then(|| {
            slices.iter().map(|slice| {
                Row::Cells(vec![
                    Cell::spans([
                        ("    ", Style::new()),
                        ("■", color(slice.category)),
                        (" ", Style::new()),
                        (label(slice.category), Style::new()),
                    ]),
                    Cell::new(slice.bytes),
                    Cell::new(percent(slice.bytes, total)),
                ])
            })
        });
        std::iter::once(group).chain(categories.into_iter().flatten())
    });
    let overlap = match breakdown.remainder {
        Remainder::Overcounted(extra) if extra >= Bytes(64 << 20) => Some(Row::Cells(vec![
            Cell::new("  Counted twice (overlapping counters)"),
            Cell::new(extra),
        ])),
        _ => None,
    };
    // MemAvailable is the kernel's estimate: free memory and droppable cache, minus the
    // reserves it keeps for itself. Showing the difference makes the rows add up.
    let free_and_cache = group_sum(Reclaim::Free) + group_sum(Reclaim::Cache);
    let held_back = match mem.available.cmp(&free_and_cache) {
        _ if mem.available.0.abs_diff(free_and_cache.0) < Bytes::MIB.0 => None,
        std::cmp::Ordering::Less => Some(entry(
            0,
            "Held back by the kernel",
            format!("−{}", free_and_cache.saturating_sub(mem.available)),
        )),
        _ => Some(entry(
            0,
            "Other reclaimable memory",
            format!("+{}", mem.available.saturating_sub(free_and_cache)),
        )),
    };
    let available = match snapshot.arc.available() {
        Some(arc) => vec![
            entry(0, "Available", mem.available),
            entry(0, "ZFS ARC can shrink by", arc.shrinkable()),
            entry(0, "Total available", mem.available + arc.shrinkable()).bold(),
        ],
        None => vec![entry(0, "Available", mem.available).bold()],
    };
    let classic =
        detailed.then(|| entry(0, "\"used\" as classic `free` reports it", mem.classic_used()));

    // One table, so amounts line up from the shares down to the total.
    let rows = group_rows
        .chain(overlap)
        .chain([Row::Cells(vec![])])
        .chain(held_back.into_iter().chain(available).chain(classic).map(Entry::into_row));
    let table = Table::new(
        vec![Column::left(""), Column::right(""), Column::right(""), Column::help("")],
        rows.collect(),
    );
    Section::new("OVERVIEW", [Block::Line(bar), Block::Table(table)])
}

/// One line of the programs table: a program (or a single process) and its memory.
struct ProgramRow {
    pid: u32,
    name: String,
    processes: Option<usize>,
    command: Option<String>,
    mem: Rollup,
}

fn programs(snapshot: &Snapshot, options: &Options) -> Section {
    let scan = &snapshot.processes;
    let rows: Vec<ProgramRow> = if options.per_process {
        sorted_by_key(scan.processes.iter().collect(), |p| Reverse(p.mem.pss))
            .into_iter()
            .map(|p| ProgramRow {
                pid: p.pid,
                name: p.name.clone(),
                processes: None,
                command: Some(truncate(&p.cmdline, 48)).filter(|c| !c.is_empty()),
                mem: p.mem,
            })
            .collect()
    } else {
        procs::by_program(&scan.processes)
            .into_iter()
            .map(|group| ProgramRow {
                pid: group.root.pid,
                name: group.name.to_owned(),
                processes: Some(group.processes).filter(|&n| n > 1),
                command: None,
                mem: group.mem,
            })
            .collect()
    };
    let (shown, rest) = rows.split_at(options.programs.unwrap_or(rows.len()).min(rows.len()));
    let rest_sum: Rollup = rest.iter().map(|r| r.mem).sum();
    let all: Rollup = rows.iter().map(|r| r.mem).sum();

    let body = shown.iter().map(|r| {
        Row::Cells(vec![
            Cell::new(r.pid),
            Cell::new(truncate(&r.name, 32)),
            r.processes.map_or_else(Cell::default, Cell::new),
            Cell::new(r.mem.pss),
            Cell::new(r.mem.swap),
            r.command.as_deref().map_or_else(Cell::default, |c| Cell::styled(c, DIM)),
        ])
    });
    let summary_row = |name: String, mem: &Rollup| {
        vec![
            Cell::default(),
            Cell::new(name),
            Cell::default(),
            Cell::new(mem.pss),
            Cell::new(mem.swap),
        ]
    };
    let rest_row = (!rest.is_empty())
        .then(|| Row::styled(summary_row(format!("…{} more", rest.len()), &rest_sum), DIM));
    let total_row =
        Row::footer(summary_row(format!("All {} processes", scan.processes.len()), &all));
    let table = Table::new(
        vec![
            Column::right("PID"),
            Column::left("Program"),
            Column::right("Procs"),
            Column::right("RAM"),
            Column::right("Swap"),
            Column::left("Command"),
        ],
        body.chain(rest_row).chain([Row::Rule, total_row]).collect(),
    );
    let unreadable = match scan.unreadable {
        0 => None,
        1 => Some("1 process of another user unreadable; run with sudo to include it".to_owned()),
        n => {
            Some(format!("{n} processes of other users unreadable; run with sudo to include them"))
        }
    };
    Section::new(
        "PROGRAMS",
        [Block::Table(table)].into_iter().chain(unreadable.map(Block::Warning)),
    )
}

fn zfs(snapshot: &Snapshot) -> Option<Section> {
    let arc = match &snapshot.arc {
        Source::Available(arc) => arc,
        Source::Absent => return None,
        other => return Some(Section::new("ZFS ARC", source_warning(other))),
    };
    let s = &arc.sizes;
    let filled = allocate_cells(&[s.size.0, s.max.saturating_sub(s.size).0], 30);
    let gauge = format!(
        "{}{}",
        paint(color(Category::ZfsArc), "█".repeat(filled[0])),
        "░".repeat(filled[1])
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
    Some(Section::new("ZFS ARC", [Block::Line(gauge), entries(rows.into_iter().flatten())]))
}

fn kernel(snapshot: &Snapshot, slabs: usize) -> Section {
    let mem = &snapshot.meminfo;
    let counters = entries([
        entry(0, "Kernel objects (unreclaimable slab)", mem.slab_unreclaimable)
            .help("must stay in RAM"),
        entry(0, "Kernel caches (reclaimable slab)", mem.slab_reclaimable)
            .help("dropped under memory pressure"),
        entry(0, "Page tables", mem.page_tables + mem.secondary_page_tables)
            .help("map programs' memory onto physical RAM"),
        entry(0, "Kernel stacks", mem.kernel_stack).help("one per thread"),
        entry(0, "Per-CPU data", mem.percpu),
        entry(0, "vmalloc", mem.vmalloc_used).help("large kernel buffers; overlaps the above"),
    ]);
    let slab_blocks = match &snapshot.slabs {
        Source::Available(caches) => slab_blocks(caches, slabs),
        other => source_warning(other),
    };
    Section::new("KERNEL", std::iter::once(counters).chain(slab_blocks))
}

fn slab_blocks(caches: &[SlabCache], limit: usize) -> Vec<Block> {
    let caches = sorted_by_key(caches.to_vec(), |cache| Reverse(cache.footprint));
    let footprint: Bytes = caches.iter().map(|c| c.footprint).sum();
    let in_use: Bytes = caches.iter().map(SlabCache::in_use).sum();
    let wasted: Bytes = caches.iter().map(SlabCache::wasted).sum();
    let wasted_unreclaimable: Bytes =
        caches.iter().filter(|c| c.reclaimable == Some(false)).map(SlabCache::wasted).sum();

    let body = caches.iter().take(limit).map(|cache| {
        let freeable = match cache.reclaimable {
            Some(true) => "yes",
            Some(false) => "no",
            None => "?",
        };
        Row::Cells(vec![
            Cell::new(&cache.name),
            Cell::new(cache.footprint),
            Cell::new(percent(cache.in_use(), cache.footprint)),
            Cell::new(cache.wasted()),
            Cell::new(freeable),
            Cell::new(cache.description().unwrap_or("")),
        ])
    });
    let total = Row::footer(vec![
        Cell::new(format!("All {} caches", caches.len())),
        Cell::new(footprint),
        Cell::new(percent(in_use, footprint)),
        Cell::new(wasted),
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
        body.chain([Row::Rule, total]).collect(),
    );
    let fragmentation = [
        Some(
            entry(0, "Slab fragmentation", wasted)
                .detail(format!("{} of slab", percent(wasted, footprint)))
                .help("empty space inside partly filled slabs"),
        ),
        caches.iter().any(|c| c.reclaimable.is_some()).then(|| {
            entry(2, "in unreclaimable caches", wasted_unreclaimable)
                .help("stuck until neighbouring objects are freed")
        }),
    ];
    vec![Block::Table(table), entries(fragmentation.into_iter().flatten())]
}

fn cache(snapshot: &Snapshot, breakdown: &Breakdown) -> Section {
    let mem = &snapshot.meminfo;
    let mounts = snapshot
        .tmpfs
        .iter()
        .filter(|mount| mount.used >= NOTICEABLE)
        .take(8)
        .map(|mount| entry(2, &truncate(&mount.path, 36), mount.used));
    let in_mounts: Bytes = snapshot.tmpfs.iter().map(|mount| mount.used).sum();
    let elsewhere = mem.shmem.saturating_sub(in_mounts);
    let file_cache = entries([
        entry(0, "File cache", breakdown.bytes(Category::FileCache))
            .help("copies of files read from disk"),
        entry(2, "recently used", mem.active_file).help("kept longest"),
        entry(2, "not recently used", mem.inactive_file).help("dropped first"),
        entry(2, "mapped by running programs", mem.mapped).help("code, libraries, mmapped files"),
        entry(2, "not yet written to disk", mem.dirty + mem.writeback)
            .help("written out before it can be dropped"),
    ]);
    let shared =
        entries(
            std::iter::once(
                entry(0, "Shared memory & tmpfs", mem.shmem)
                    .help("can't be dropped; `free` calls it cache"),
            )
            .chain(mounts)
            .chain((elsewhere >= NOTICEABLE).then(|| {
                entry(2, "elsewhere", elsewhere).help("SysV/POSIX shm, memfd, GPU buffers")
            })),
        );
    Section::new("FILE CACHE & SHARED MEMORY", [file_cache, shared])
}

fn swap(snapshot: &Snapshot) -> Option<Section> {
    let mem = &snapshot.meminfo;
    let zram: Vec<_> = snapshot.zram.iter().filter(|z| z.original > Bytes::ZERO).collect();
    if mem.swap_total == Bytes::ZERO && zram.is_empty() {
        return None;
    }
    let devices = snapshot.swaps.available().into_iter().flatten().map(|device| {
        let kind = if device.path.contains("zram") { "zram" } else { device.kind.as_str() };
        entry(2, &format!("{} ({kind})", truncate(&device.path, 30)), device.used)
            .detail(format!("of {}", device.size))
    });
    let compression = zram.iter().map(|z| {
        let ratio = z.original.0 as f64 / z.ram_used.0.max(1) as f64;
        entry(2, &format!("{} RAM used", z.name), z.ram_used)
            .detail(format!("holds {}, {ratio:.1}× compressed", z.original))
    });
    let swap_used = mem.swap_total.saturating_sub(mem.swap_free);
    let rows = std::iter::once(
        entry(0, "Swap in use", swap_used).detail(format!("of {}", mem.swap_total)),
    )
    .chain(devices)
    .chain(compression)
    .chain([entry(0, "Swap cache", mem.swap_cached).help("pages kept both in RAM and in swap")]);
    let warning = match &snapshot.swaps {
        Source::Available(_) | Source::Absent => vec![],
        other => source_warning(other),
    };
    Some(Section::new("SWAP", std::iter::once(entries(rows)).chain(warning)))
}

fn fragmentation(snapshot: &Snapshot) -> Option<Section> {
    let zones = match &snapshot.buddy {
        Source::Available(zones) => zones,
        Source::Absent => return None,
        other => return Some(Section::new("FREE MEMORY FRAGMENTATION", source_warning(other))),
    };
    let split = buddy::free_by_block_size(zones, snapshot.page_size);
    let free: Bytes = split.iter().map(|(_, bytes)| *bytes).sum();
    if free < Bytes(64 << 20) {
        return None;
    }
    let large = split
        .iter()
        .find(|(size, _)| *size == BlockSize::Large)
        .map_or(Bytes::ZERO, |(_, bytes)| *bytes);
    let verdict = match large.fraction_of(free) {
        f if f >= 0.5 => "healthy",
        f if f >= 0.2 => "getting fragmented",
        _ => "fragmented: huge pages need compaction",
    };
    let rows = split.iter().map(|(size, bytes)| {
        let row = entry(0, &format!("Free {}", size.label()), *bytes).detail(percent(*bytes, free));
        if *size == BlockSize::Large { row.help(verdict) } else { row }
    });
    Some(Section::new("FREE MEMORY FRAGMENTATION", [entries(rows)]))
}

fn unexplained(snapshot: &Snapshot, breakdown: &Breakdown) -> Option<Section> {
    let mem = &snapshot.meminfo;
    let amount = breakdown.unexplained();
    let vmalloc = mem.vmalloc_used.saturating_sub(mem.kernel_stack);
    let rows = [
        Some(entry(0, "Not covered by any kernel counter", amount)),
        (vmalloc >= NOTICEABLE).then(|| {
            entry(2, "vmalloc (excluding kernel stacks)", vmalloc).help("possible source")
        }),
        (mem.balloon >= NOTICEABLE)
            .then(|| entry(2, "VM balloon", mem.balloon).help("possible source")),
        Some(
            entry(2, "pages drivers allocate directly", "?")
                .help("e.g. NVIDIA, i915; not reported"),
        ),
    ];
    (amount >= NOTICEABLE)
        .then(|| Section::new("UNEXPLAINED MEMORY", [entries(rows.into_iter().flatten())]))
}

/// The warning for a source that couldn't be used, if it was there to begin with.
fn source_warning<T>(source: &Source<T>) -> Vec<Block> {
    match source {
        Source::Available(_) | Source::Absent => vec![],
        Source::NeedsRoot => vec![Block::Warning("needs root; run with sudo".to_owned())],
        Source::Broken(reason) => vec![Block::Warning(format!("couldn't read {reason}"))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_add_up_to_width() {
        assert_eq!(allocate_cells(&[1, 1, 1], 10), vec![4, 3, 3]);
        assert_eq!(allocate_cells(&[0, 5], 4), vec![0, 4]);
        assert_eq!(allocate_cells(&[0, 0], 4), vec![0, 0]);
        let cells = allocate_cells(&[7, 13, 29, 1, 50], 72);
        assert_eq!(cells.iter().sum::<usize>(), 72);
    }

    /// A 64 GiB ZFS workstation with zram swap, assembled from hand-written kernel files.
    fn zfs_workstation() -> Snapshot {
        use crate::procs::{Process, ProcessScan};
        use crate::swap::{SwapDevice, Zram};
        use crate::{buddy, meminfo, slab, zfs};

        const GIB: u64 = 1 << 30;
        let meminfo = meminfo::parse(
            "MemTotal: 65536000 kB\nMemFree: 4000000 kB\nMemAvailable: 12000000 kB\n\
             Buffers: 100000 kB\nCached: 9000000 kB\nSwapCached: 50000 kB\n\
             Active(file): 5000000 kB\nInactive(file): 2000000 kB\nSwapTotal: 8388604 kB\n\
             SwapFree: 6000000 kB\nDirty: 3000 kB\nAnonPages: 20000000 kB\nMapped: 900000 kB\n\
             Shmem: 2000000 kB\nSReclaimable: 1500000 kB\nSUnreclaim: 4000000 kB\n\
             KernelStack: 40000 kB\nPageTables: 300000 kB\nVmallocUsed: 600000 kB\n\
             Percpu: 20000 kB\nHugetlb: 0 kB\n",
        )
        .unwrap();
        let arc = zfs::parse(&format!(
            "header\nname type data\nhits 4 9800\nmisses 4 200\nsize 4 {}\nc 4 {}\n\
             c_min 4 {}\nc_max 4 {}\ndata_size 4 {}\nmetadata_size 4 {}\nhdr_size 4 {}\n\
             dbuf_size 4 {}\ndnode_size 4 {}\nbonus_size 4 {}\n",
            22 * GIB,
            22 * GIB,
            2 * GIB,
            32 * GIB,
            17 * GIB,
            3 * GIB,
            GIB / 2,
            GIB / 2,
            GIB,
            0
        ))
        .unwrap();
        let slabs = slab::parse(
            "slabinfo - version: 2.1\n\
             dnode_t 900000 1000000 896 36 8 : tunables 0 0 0 : slabdata 27000 27778 0\n\
             kmalloc-64 9000000 16000000 64 64 1 : tunables 0 0 0 : slabdata 250000 250000 0\n\
             dentry 3000000 3100000 192 21 1 : tunables 0 0 0 : slabdata 147000 147619 0\n",
            Bytes(4096),
            |name| Some(name == "dentry"),
        )
        .unwrap();
        let process = |pid, ppid, name: &str, gib_pss: u64| Process {
            pid,
            ppid,
            started: u64::from(pid),
            name: name.to_owned(),
            cmdline: format!("/usr/bin/{name} --flag"),
            mem: Rollup { pss: Bytes(gib_pss * GIB), swap: Bytes(gib_pss * GIB / 20) },
        };
        Snapshot {
            hostname: "workstation".to_owned(),
            kernel: "6.12.0".to_owned(),
            page_size: Bytes(4096),
            meminfo,
            arc: Source::Available(arc),
            slabs: Source::Available(slabs),
            processes: ProcessScan {
                processes: vec![
                    process(100, 1, "firefox", 6),
                    process(101, 100, "firefox", 3),
                    process(2909719, 1, "java", 5),
                    process(300, 1, "Xorg", 1),
                ],
                unreadable: 3,
            },
            buddy: Source::Available(
                buddy::parse("Node 0, zone Normal 90000 40000 9000 2000 500 100 20 4 1 0 0\n")
                    .unwrap(),
            ),
            swaps: Source::Available(vec![SwapDevice {
                path: "/dev/zram0".to_owned(),
                kind: "partition".to_owned(),
                size: Bytes(8 * GIB),
                used: Bytes(2 * GIB),
            }]),
            zram: vec![Zram {
                name: "zram0".to_owned(),
                original: Bytes(2 * GIB),
                ram_used: Bytes(GIB / 2),
            }],
            zsmalloc_pool: Bytes(GIB / 2),
            tmpfs: vec![],
            gpus: vec![],
        }
    }

    fn options(sections: &[SectionKind], detailed: bool) -> Options {
        Options {
            sections: sections.to_vec(),
            detailed,
            programs: Some(2),
            slabs: 5,
            per_process: false,
            show: Show { headers: true, help: true },
        }
    }

    fn strip_ansi(text: &str) -> String {
        text.split('\u{1b}')
            .enumerate()
            .map(
                |(i, part)| {
                    if i == 0 { part } else { part.split_once('m').map_or(part, |(_, rest)| rest) }
                },
            )
            .collect()
    }

    #[test]
    fn summary_shows_sums_and_programs_only() {
        let text = strip_ansi(&render(&zfs_workstation(), &options(&SectionKind::SUMMARY, false)));
        println!("{text}");
        assert!(text.contains("Total available"));
        assert!(text.contains("31.4 GiB")); // 11.4 GiB MemAvailable + 20.0 GiB ARC shrink
        assert!(text.contains("Held back by the kernel"));
        assert!(text.contains("2909719  java"));
        assert!(text.contains("100  firefox"));
        assert!(text.contains("…1 more"));
        assert!(!text.contains("Page tables"));
        assert!(!text.contains("KERNEL"));
    }

    #[test]
    fn detailed_report_covers_every_source() {
        let snapshot = zfs_workstation();
        let text = strip_ansi(&render(&snapshot, &options(&SectionKind::ALL, true)));
        println!("{text}");
        let breakdown = Breakdown::of(&snapshot);
        // ARC minus the bookkeeping already counted as slab.
        assert_eq!(breakdown.bytes(Category::ZfsArc), Bytes(20 << 30));
        assert_eq!(breakdown.bytes(Category::CompressedSwap), Bytes(512 << 20));
        for section in ["ZFS ARC", "KERNEL", "SWAP", "FRAGMENTATION", "UNEXPLAINED"] {
            assert!(text.contains(section), "missing {section}");
        }
        assert!(text.contains("Page tables"));
        assert!(text.contains("4.0× compressed"));
        assert!(text.contains("fragmented: huge pages need compaction"));
    }

    #[test]
    fn bare_program_list_has_no_headers_or_help() {
        let options = Options {
            programs: None,
            show: Show { headers: false, help: false },
            ..options(&[SectionKind::Programs], false)
        };
        let text = strip_ansi(&render(&zfs_workstation(), &options));
        println!("{text}");
        assert!(!text.contains("PROGRAMS"));
        assert!(!text.contains("PID"));
        assert!(!text.contains("memori ·"));
        assert!(text.contains("Xorg"));
        assert!(!text.contains("All 4 processes"));
        assert!(!text.contains("unreadable"));
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
