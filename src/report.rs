//! Turns a [`Snapshot`] into the text report.
//!
//! Every section is a function returning its lines; sections with nothing to say return
//! no lines and are skipped.

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
use crate::units::Bytes;
use crate::zfs::ArcStats;

pub struct Options {
    /// How many programs to list.
    pub programs: usize,
    /// How many slab caches to list.
    pub slabs: usize,
    /// List individual processes instead of grouping them by program.
    pub per_process: bool,
}

pub fn render(snapshot: &Snapshot, options: &Options) -> String {
    let breakdown = Breakdown::of(snapshot);
    let sections = [
        header(snapshot),
        overview(snapshot, &breakdown),
        programs(snapshot, options),
        zfs(snapshot),
        kernel(snapshot, options),
        caches(snapshot, &breakdown),
        swap(snapshot),
        fragmentation(snapshot),
        unexplained(snapshot, &breakdown),
        notes(snapshot),
    ];
    let body: Vec<String> = sections
        .into_iter()
        .filter(|lines| !lines.is_empty())
        .map(|lines| lines.join("\n"))
        .collect();
    format!("{}\n", body.join("\n\n"))
}

// ─── Layout ──────────────────────────────────────────────────────────────────────────────

const WIDTH: usize = 80;
/// Width of the label column in "label  amount  note" rows, including indentation.
const LABEL: usize = 46;
const BAR: usize = 72;
/// Amounts smaller than this are left out of lists.
const NOTICEABLE: Bytes = Bytes::MIB;

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const WARN: Style = fg(AnsiColor::Yellow).bold();

const fn fg(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(color)))
}

fn paint(style: Style, text: impl Display) -> String {
    format!("{style}{text}{style:#}")
}

fn heading(title: &str) -> String {
    let rule = "━".repeat(WIDTH.saturating_sub(title.chars().count() + 4));
    paint(BOLD, format!("━━ {title} {rule}"))
}

/// Explanatory text: word-wrapped and dimmed.
fn prose(indent: usize, text: &str) -> Vec<String> {
    wrap(indent, text).into_iter().map(|line| paint(DIM, line)).collect()
}

/// Something the reader should act on: word-wrapped and highlighted.
fn warning(text: &str) -> Vec<String> {
    let lines = wrap(4, text);
    let first = lines.first().map(|line| format!("  ⚠ {}", line.trim_start()));
    first.into_iter().chain(lines.into_iter().skip(1)).map(|line| paint(WARN, line)).collect()
}

/// Greedy word wrap to `WIDTH`, each line indented by `indent`.
fn wrap(indent: usize, text: &str) -> Vec<String> {
    text.split_whitespace().fold(Vec::<String>::new(), |mut lines, word| {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= WIDTH => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(format!("{:indent$}{word}", "")),
        }
        lines
    })
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

/// `indent label ........ amount  note`. `label` may contain styling, so its visible
/// width is measured from `plain`.
fn styled_row(plain: &str, styled: String, amount: impl Display, note: &str) -> String {
    let padding = " ".repeat(LABEL.saturating_sub(plain.chars().count()));
    let note = if note.is_empty() { String::new() } else { format!("  {}", paint(DIM, note)) };
    format!("{styled}{padding}{amount:>10}{note}")
}

fn row(indent: usize, label: &str, amount: impl Display, note: &str) -> String {
    let plain = format!("{:indent$}{label}", "");
    styled_row(&plain, plain.clone(), amount, note)
}

// ─── Category presentation ───────────────────────────────────────────────────────────────

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
        Category::Unexplained => "Unexplained (no counter covers it)",
        Category::ZfsArc => "ZFS ARC (ZFS's own disk cache)",
        Category::FileCache => "File cache (copies of files on disk)",
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
fn glyph(reclaim: Reclaim) -> char {
    match reclaim {
        Reclaim::InUse => '█',
        Reclaim::Shrinkable => '▓',
        Reclaim::Cache => '▒',
        Reclaim::Free => '░',
    }
}

fn title(reclaim: Reclaim) -> (&'static str, &'static str) {
    match reclaim {
        Reclaim::InUse => ("In use", "freed only by quitting programs"),
        Reclaim::Shrinkable => ("Shrinkable", "given back when RAM runs low"),
        Reclaim::Cache => ("Cache", "dropped instantly when needed"),
        Reclaim::Free => ("Free", "not used for anything"),
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

fn header(snapshot: &Snapshot) -> Vec<String> {
    let line = format!(
        "memori · {} · Linux {} · {} of RAM",
        snapshot.hostname, snapshot.kernel, snapshot.meminfo.total
    );
    vec![paint(BOLD, line)]
}

fn overview(snapshot: &Snapshot, breakdown: &Breakdown) -> Vec<String> {
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

    let ordered: Vec<Slice> = groups.iter().flat_map(|(_, slices)| slices.clone()).collect();
    let cells = allocate_cells(&ordered.iter().map(|s| s.bytes.0).collect::<Vec<_>>(), BAR);
    let bar: String = ordered
        .iter()
        .zip(cells)
        .filter(|(_, n)| *n > 0)
        .map(|(slice, n)| {
            let glyphs = glyph(slice.category.reclaim()).to_string().repeat(n);
            paint(color(slice.category), glyphs)
        })
        .collect();

    let group_rows = groups.iter().flat_map(|(reclaim, slices)| {
        let (name, meaning) = title(*reclaim);
        let sum: Bytes = slices.iter().map(|s| s.bytes).sum();
        let plain = format!("  {} {name} · {meaning}", glyph(*reclaim));
        let styled = format!(
            "  {} {} {}",
            glyph(*reclaim),
            paint(BOLD, name),
            paint(DIM, format!("· {meaning}"))
        );
        let group_row = styled_row(&plain, styled, sum, &format!("{:>4}", percent(sum, total)));
        // A group of one category says everything in its own row.
        let category_rows = (slices.len() > 1 || *reclaim == Reclaim::Shrinkable).then(|| {
            slices.iter().map(|slice| {
                let name = label(slice.category);
                let plain = format!("      ■ {name}");
                let styled = format!("      {} {name}", paint(color(slice.category), "■"));
                let share = format!("{:>4}", percent(slice.bytes, total));
                styled_row(&plain, styled, slice.bytes, &share)
            })
        });
        std::iter::once(group_row).chain(category_rows.into_iter().flatten())
    });

    let arc_extra = snapshot.arc.available().map(ArcStats::shrinkable).filter(|b| *b >= NOTICEABLE);
    let available = [
        Some(row(
            2,
            "Available for new programs",
            mem.available,
            "free + droppable cache (MemAvailable)",
        )),
        arc_extra.map(|extra| row(4, "+ if the ZFS ARC shrinks to its minimum", extra, "")),
        Some(row(
            2,
            "\"used\" as classic `free` reports it",
            mem.classic_used(),
            "total − free − buff/cache",
        )),
    ];
    let overlap = match breakdown.remainder {
        Remainder::Overcounted(extra) if extra >= Bytes(64 << 20) => prose(
            2,
            &format!(
                "Note: some of the kernel's counters overlap by about {extra} (typically ZFS \
                 buffers that are also counted as kernel slab), so the categories add up to a \
                 little more than your total RAM."
            ),
        ),
        _ => vec![],
    };

    [
        vec![heading("WHERE YOUR RAM IS"), String::new(), format!("  {bar}"), String::new()],
        group_rows.collect(),
        vec![String::new()],
        available.into_iter().flatten().collect(),
        overlap,
    ]
    .concat()
}

/// One line of the programs table: a program (or a single process) and its memory.
struct ProgramRow {
    name: String,
    who: String,
    detail: Option<String>,
    mem: Rollup,
}

fn programs(snapshot: &Snapshot, options: &Options) -> Vec<String> {
    let scan = &snapshot.processes;
    let rows: Vec<ProgramRow> = if options.per_process {
        let processes = sorted_by_key(scan.processes.iter().collect(), |p| Reverse(p.mem.pss));
        processes
            .into_iter()
            .map(|p| ProgramRow {
                name: p.name.clone(),
                who: format!("pid {}", p.pid),
                detail: Some(p.cmdline.clone()).filter(|c| !c.is_empty()),
                mem: p.mem,
            })
            .collect()
    } else {
        procs::by_program(&scan.processes)
            .into_iter()
            .map(|group| ProgramRow {
                name: group.name.to_owned(),
                who: match group.members[..] {
                    [only] => format!("pid {}", only.pid),
                    _ => format!("{} procs", group.members.len()),
                },
                detail: None,
                mem: group.mem,
            })
            .collect()
    };
    let (shown, rest) = rows.split_at(options.programs.min(rows.len()));
    let rest_sum: Rollup = rest.iter().map(|r| r.mem).sum();
    let all: Rollup = rows.iter().map(|r| r.mem).sum();

    let line = |name: &str, who: &str, mem: &Rollup| {
        format!(
            "  {:<26}{:>11}{:>11}{:>12}{:>10}",
            truncate(name, 25),
            who,
            mem.unique(),
            mem.pss,
            mem.swap
        )
    };
    let table_header = paint(
        BOLD,
        format!("  {:<26}{:>11}{:>11}{:>12}{:>10}", "Program", "", "Unique", "Fair share", "Swap"),
    );
    let body = shown.iter().map(|r| match &r.detail {
        Some(detail) => {
            format!("{}  {}", line(&r.name, &r.who, &r.mem), paint(DIM, truncate(detail, 28)))
        }
        None => line(&r.name, &r.who, &r.mem),
    });
    let rest_line = (!rest.is_empty())
        .then(|| paint(DIM, line(&format!("…{} more", rest.len()), "", &rest_sum)));
    let total_line =
        paint(BOLD, line(&format!("All {} processes", scan.processes.len()), "", &all));
    let composition = prose(
        4,
        &format!(
            "Fair shares in total: {} programs' own data, {} mapped files (code, libraries), \
             {} shared memory.",
            all.pss_anon, all.pss_file, all.pss_shmem
        ),
    );
    let unreadable = match scan.unreadable {
        0 => vec![],
        1 => warning(
            "1 process belongs to another user and couldn't be inspected; run with sudo to include it.",
        ),
        n => warning(&format!(
            "{n} processes belong to other users and couldn't be inspected; run with sudo to include them."
        )),
    };

    [
        vec![heading("PROGRAMS"), String::new()],
        prose(
            2,
            &format!(
                "{} Unique (USS) is memory no other process uses: roughly what you get back \
                 by quitting the program. Fair share (PSS) also splits memory shared between \
                 processes (libraries, shared buffers) evenly among them, so fair shares add \
                 up without double counting.",
                if options.per_process {
                    "Largest processes first."
                } else {
                    "Grouped by executable, largest first."
                }
            ),
        ),
        vec![String::new(), table_header],
        body.collect(),
        rest_line.into_iter().collect(),
        vec![format!("  {}", "─".repeat(68)), total_line],
        composition,
        unreadable,
    ]
    .concat()
}

fn zfs(snapshot: &Snapshot) -> Vec<String> {
    let Some(arc) = snapshot.arc.available() else {
        return vec![];
    };
    let s = &arc.sizes;
    let filled = allocate_cells(&[s.size.0, s.max.saturating_sub(s.size).0], 30);
    let gauge = format!(
        "[{}{}] of {} max",
        paint(color(Category::ZfsArc), "█".repeat(filled[0])),
        "░".repeat(filled[1]),
        s.max
    );
    let hit_rate = arc.hit_ratio().map(|ratio| {
        row(2, "Hit rate", format!("{:.1}%", 100.0 * ratio), "reads served from RAM since boot")
    });
    [
        vec![heading("ZFS ARC"), String::new()],
        prose(
            2,
            "ZFS keeps its own disk cache, the ARC, instead of using the kernel's file cache, \
             so tools like `free` count it as used. The ARC gives memory back when programs \
             need it, but never shrinks below its minimum.",
        ),
        vec![
            String::new(),
            format!("{}  {gauge}", row(2, "Size now", s.size, "")),
            row(2, "Target", s.target, "the size the ARC is currently steering toward"),
            row(2, "Minimum", s.min, &format!("so it can give back up to {}", arc.shrinkable())),
            "  Holding".to_owned(),
            row(4, "file data", s.data, ""),
            row(4, "metadata", s.metadata, ""),
            row(4, "bookkeeping", arc.bookkeeping(), "headers, dnodes, dbufs"),
        ],
        prose(
            6,
            "Bookkeeping lives in kernel slab caches, so the overview counts it there rather \
             than under ZFS ARC.",
        ),
        hit_rate.into_iter().collect(),
    ]
    .concat()
}

fn kernel(snapshot: &Snapshot, options: &Options) -> Vec<String> {
    let mem = &snapshot.meminfo;
    let counters = [
        row(2, "Kernel objects (unreclaimable slab)", mem.slab_unreclaimable, "must stay in RAM"),
        row(
            2,
            "Kernel caches (reclaimable slab)",
            mem.slab_reclaimable,
            "dropped under memory pressure",
        ),
        row(
            2,
            "Page tables",
            mem.page_tables + mem.secondary_page_tables,
            "map programs' memory onto physical RAM",
        ),
        row(2, "Kernel stacks", mem.kernel_stack, "one small stack per thread"),
        row(2, "Per-CPU data", mem.percpu, ""),
        row(2, "vmalloc", mem.vmalloc_used, "large kernel buffers; overlaps the above"),
    ];
    let slabs = match &snapshot.slabs {
        Source::Available(caches) => slab_table(caches, options.slabs),
        Source::NeedsRoot => [
            vec![String::new()],
            warning(
                "Run with sudo to see which kernel caches hold the slab memory (like slabtop).",
            ),
        ]
        .concat(),
        Source::Absent | Source::Broken(_) => vec![],
    };
    [vec![heading("KERNEL"), String::new()], counters.to_vec(), slabs].concat()
}

fn slab_table(caches: &[SlabCache], limit: usize) -> Vec<String> {
    let caches = sorted_by_key(caches.to_vec(), |cache| Reverse(cache.footprint));
    let footprint: Bytes = caches.iter().map(|c| c.footprint).sum();
    let in_use: Bytes = caches.iter().map(SlabCache::in_use).sum();
    let wasted: Bytes = caches.iter().map(SlabCache::wasted).sum();
    let wasted_unreclaimable: Bytes =
        caches.iter().filter(|c| c.reclaimable == Some(false)).map(SlabCache::wasted).sum();

    let line = |name: &str, size: Bytes, used: String, wasted: Bytes, freeable: &str| {
        let text = format!(
            "  {:<24}{:>10}{:>7}{:>10}{:>10}",
            truncate(name, 23),
            size,
            used,
            wasted,
            freeable
        );
        text.trim_end().to_owned()
    };
    let table_header = paint(
        BOLD,
        format!(
            "  {:<24}{:>10}{:>7}{:>10}{:>10}  {}",
            "Cache", "Size", "Full", "Wasted", "Freeable", "What it holds"
        ),
    );
    let body = caches.iter().take(limit).map(|cache| {
        let freeable = match cache.reclaimable {
            Some(true) => "yes",
            Some(false) => "no",
            None => "?",
        };
        let full = percent(cache.in_use(), cache.footprint);
        let text = line(&cache.name, cache.footprint, full, cache.wasted(), freeable);
        format!("{text}  {}", paint(DIM, cache.description().unwrap_or("")))
    });
    let total_line = paint(
        BOLD,
        line(
            &format!("All {} caches", caches.len()),
            footprint,
            percent(in_use, footprint),
            wasted,
            "",
        ),
    );
    let reclaim_known = caches.iter().any(|c| c.reclaimable.is_some());
    let unreclaimable_clause = if reclaim_known {
        format!(
            " {wasted_unreclaimable} of it is in unreclaimable caches, where it can stay stuck \
             until the objects sharing those slabs are freed."
        )
    } else {
        String::new()
    };

    [
        vec![String::new()],
        prose(2, "Slab caches, largest first (what `slabtop -s c` shows):"),
        vec![table_header],
        body.collect(),
        vec![format!("  {}", "─".repeat(61)), total_line],
        prose(
            2,
            &format!(
                "Fragmentation: {wasted} ({}) of slab memory holds no objects; it is empty \
                 space inside partly filled slabs.{unreclaimable_clause}",
                percent(wasted, footprint)
            ),
        ),
    ]
    .concat()
}

fn caches(snapshot: &Snapshot, breakdown: &Breakdown) -> Vec<String> {
    let mem = &snapshot.meminfo;
    let mounts: Vec<String> = snapshot
        .tmpfs
        .iter()
        .filter(|mount| mount.used >= NOTICEABLE)
        .take(8)
        .map(|mount| row(4, &truncate(&mount.path, 36), mount.used, ""))
        .collect();
    let in_mounts: Bytes = snapshot.tmpfs.iter().map(|mount| mount.used).sum();
    let elsewhere = mem.shmem.saturating_sub(in_mounts);
    [
        vec![
            heading("FILE CACHE & SHARED MEMORY"),
            String::new(),
            row(
                2,
                "File cache",
                breakdown.bytes(Category::FileCache),
                "copies of files read from disk",
            ),
            row(4, "recently used", mem.active_file, "kept longest"),
            row(4, "not recently used", mem.inactive_file, "dropped first"),
            row(4, "mapped by running programs", mem.mapped, "code, libraries, mmapped files"),
            row(
                4,
                "not yet written to disk",
                mem.dirty + mem.writeback,
                "written out before it can be dropped",
            ),
            String::new(),
            row(2, "Shared memory & tmpfs", mem.shmem, "lives only in RAM: can't be dropped"),
        ],
        mounts,
        (elsewhere >= NOTICEABLE)
            .then(|| row(4, "elsewhere", elsewhere, "shared memory segments, memfd, GPU buffers"))
            .into_iter()
            .collect(),
        prose(
            2,
            "`free` counts shared memory as buff/cache, but unlike the file cache it can only \
             be freed by deleting the files or swapping it out.",
        ),
    ]
    .concat()
}

fn swap(snapshot: &Snapshot) -> Vec<String> {
    let mem = &snapshot.meminfo;
    let zram: Vec<_> = snapshot.zram.iter().filter(|z| z.original > Bytes::ZERO).collect();
    if mem.swap_total == Bytes::ZERO && zram.is_empty() {
        return vec![];
    }
    let devices: Vec<String> = snapshot
        .swaps
        .available()
        .into_iter()
        .flatten()
        .map(|device| {
            let kind = if device.path.contains("zram") { "zram" } else { device.kind.as_str() };
            let label = format!("{} ({kind})", truncate(&device.path, 30));
            row(4, &label, device.used, &format!("of {}", device.size))
        })
        .collect();
    let compression = zram.iter().map(|z| {
        let ratio = z.original.0 as f64 / z.ram_used.0.max(1) as f64;
        format!(
            "{} holds {} of swapped data compressed into {} of RAM ({ratio:.1}× smaller).",
            z.name, z.original, z.ram_used
        )
    });
    let groups = procs::by_program(&snapshot.processes.processes);
    let swapped = sorted_by_key(groups, |g| Reverse(g.mem.swap));
    let biggest: Vec<String> = swapped
        .iter()
        .filter(|g| g.mem.swap >= NOTICEABLE)
        .take(5)
        .map(|g| format!("{} {}", g.name, g.mem.swap))
        .collect();
    [
        vec![
            heading("SWAP"),
            String::new(),
            row(
                2,
                "Swap in use",
                mem.swap_total.saturating_sub(mem.swap_free),
                &format!("of {}", mem.swap_total),
            ),
        ],
        devices,
        compression.flat_map(|text| prose(2, &text)).collect(),
        vec![row(2, "Swap cache", mem.swap_cached, "pages kept both in RAM and in swap")],
        if biggest.is_empty() {
            vec![]
        } else {
            prose(2, &format!("Most swapped out: {}.", biggest.join(", ")))
        },
    ]
    .concat()
}

fn fragmentation(snapshot: &Snapshot) -> Vec<String> {
    let Some(zones) = snapshot.buddy.available() else {
        return vec![];
    };
    let split = buddy::free_by_block_size(zones, snapshot.page_size);
    let free: Bytes = split.iter().map(|(_, bytes)| *bytes).sum();
    if free < Bytes(64 << 20) {
        return vec![];
    }
    let large =
        split.iter().find(|(size, _)| *size == BlockSize::Large).map_or(Bytes::ZERO, |(_, b)| *b);
    let verdict = match large.fraction_of(free) {
        f if f >= 0.5 => "Healthy: most free memory sits in large blocks.",
        f if f >= 0.2 => "Somewhat fragmented: large blocks are getting scarce.",
        _ => {
            "Fragmented: little free memory is in large blocks, so huge pages and big driver \
             allocations make the kernel stop and compact memory first."
        }
    };
    [
        vec![heading("FREE MEMORY FRAGMENTATION"), String::new()],
        prose(
            2,
            "The kernel hands out free memory in contiguous blocks of 4 KiB up to 4 MiB. Huge \
             pages and some drivers need large blocks; if free memory is chopped into small \
             pieces, they can't get them without first moving memory around.",
        ),
        vec![String::new()],
        split
            .iter()
            .map(|(size, bytes)| {
                let share = format!("{:>4}", percent(*bytes, free));
                row(2, &format!("Free {}", size.label()), *bytes, &share)
            })
            .collect(),
        prose(2, verdict),
    ]
    .concat()
}

fn unexplained(snapshot: &Snapshot, breakdown: &Breakdown) -> Vec<String> {
    let mem = &snapshot.meminfo;
    let threshold = Bytes(256 << 20).max(Bytes(breakdown.total.0 / 50));
    let amount = breakdown.unexplained();
    if amount < threshold {
        return vec![];
    }
    let vmalloc = mem.vmalloc_used.saturating_sub(mem.kernel_stack);
    let suspects = [
        (vmalloc >= NOTICEABLE).then(|| {
            row(4, "vmalloc (not counting kernel stacks)", vmalloc, "large kernel buffers")
        }),
        (mem.balloon >= NOTICEABLE)
            .then(|| row(4, "VM balloon", mem.balloon, "handed back to the hypervisor")),
    ];
    [
        vec![heading("UNEXPLAINED MEMORY"), String::new()],
        prose(
            2,
            &format!(
                "{amount} of RAM isn't covered by any of the kernel's counters. Things that \
                 commonly hide there:"
            ),
        ),
        suspects.into_iter().flatten().collect(),
        prose(
            4,
            "and pages that drivers allocate directly, which the kernel doesn't count: \
             notably the NVIDIA and Intel GPU drivers, and network or storage drivers' DMA \
             buffers.",
        ),
    ]
    .concat()
}

fn notes(snapshot: &Snapshot) -> Vec<String> {
    let broken = [
        ("slab caches", broken_reason(&snapshot.slabs)),
        ("ZFS ARC", broken_reason(&snapshot.arc)),
        ("free-memory fragmentation", broken_reason(&snapshot.buddy)),
        ("swap devices", broken_reason(&snapshot.swaps)),
    ];
    let lines: Vec<String> = broken
        .into_iter()
        .filter_map(|(what, reason)| Some(warning(&format!("Couldn't read {what}: {}", reason?))))
        .flatten()
        .collect();
    if lines.is_empty() {
        return vec![];
    }
    [vec![heading("NOTES"), String::new()], lines].concat()
}

fn broken_reason<T>(source: &Source<T>) -> Option<&str> {
    match source {
        Source::Broken(reason) => Some(reason),
        _ => None,
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
        let process = |pid, name: &str, gib_pss: u64| Process {
            pid,
            name: name.to_owned(),
            cmdline: format!("/usr/bin/{name} --flag"),
            mem: Rollup {
                pss: Bytes(gib_pss * GIB),
                pss_anon: Bytes(gib_pss * GIB * 9 / 10),
                pss_file: Bytes(gib_pss * GIB / 10),
                private_dirty: Bytes(gib_pss * GIB * 8 / 10),
                swap: Bytes(gib_pss * GIB / 20),
                ..Rollup::default()
            },
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
                    process(100, "firefox", 6),
                    process(101, "firefox", 3),
                    process(200, "java", 5),
                    process(300, "Xorg", 1),
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

    #[test]
    fn renders_a_zfs_workstation() {
        let snapshot = zfs_workstation();
        let options = Options { programs: 2, slabs: 5, per_process: false };
        let text = render(&snapshot, &options);
        println!("{text}");
        let breakdown = Breakdown::of(&snapshot);
        // ARC minus the bookkeeping already counted as slab.
        assert_eq!(breakdown.bytes(Category::ZfsArc), Bytes(20 << 30));
        assert_eq!(breakdown.bytes(Category::CompressedSwap), Bytes(512 << 20));
        assert!(text.contains("ZFS ARC"));
        assert!(text.contains("2 procs"));
        assert!(text.contains("…1 more"));
        assert!(text.contains("(4.0×"));
        assert!(text.contains("Fragmented"));
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
