//! Typed facts about memory, computed from a [`Snapshot`]. Nothing here builds strings;
//! that is the render layer's job.

pub mod breakdown;

use std::cmp::Reverse;

use crate::collections::{NonEmpty, sorted_by_key};
use crate::snapshot::Snapshot;
use crate::sources::Source;
use crate::sources::buddy::{self, BlockSize};
use crate::sources::procs::{self, Process, ProcessScan, Rollup};
use crate::sources::slab::{Reclaimability, SlabCache};
use crate::sources::swap::{SwapDevice, Zram};
use crate::sources::tmpfs::TmpfsMount;
use crate::sources::zfs::ArcStats;
use crate::units::Bytes;
use breakdown::{Breakdown, Category, Reclaim, Remainder, Slice};

/// Below this, an amount isn't worth a line of its own.
pub const NOTICEABLE: Bytes = Bytes::MIB;

/// How many items of a list to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Top(usize),
    All,
}

impl Limit {
    /// The items to show, and the ones left over.
    fn split<T>(self, items: Vec<T>) -> (Vec<T>, Vec<T>) {
        let keep = match self {
            Self::Top(count) => count.min(items.len()),
            Self::All => items.len(),
        };
        let mut shown = items;
        let rest = shown.split_off(keep);
        (shown, rest)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    ByProgram,
    PerProcess,
}

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub programs: Limit,
    pub grouping: Grouping,
    pub slabs: Limit,
}

/// Every section's facts. Sections that don't apply to this system are `Absent`.
pub struct Report<'a> {
    pub host: Host<'a>,
    pub overview: Overview,
    pub programs: Programs<'a>,
    pub zfs: Source<&'a ArcStats>,
    pub kernel: Kernel<'a>,
    pub cache: Cache<'a>,
    pub swap: Option<Swap<'a>>,
    pub fragmentation: Source<Fragmentation>,
    pub unexplained: Option<Unexplained>,
}

pub fn analyze<'a>(snapshot: &'a Snapshot, settings: &Settings) -> Report<'a> {
    let breakdown = Breakdown::of(snapshot);
    Report {
        host: Host {
            hostname: snapshot.hostname.as_deref(),
            kernel: snapshot.kernel.as_deref(),
            total: snapshot.meminfo.total,
        },
        overview: overview(snapshot, &breakdown),
        programs: programs(&snapshot.processes, settings),
        zfs: snapshot.arc.as_ref(),
        kernel: kernel(snapshot, settings.slabs),
        cache: cache(snapshot, &breakdown),
        swap: swap(snapshot),
        fragmentation: fragmentation(snapshot),
        unexplained: unexplained(snapshot, &breakdown),
    }
}

// ─── Host ────────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Host<'a> {
    pub hostname: Option<&'a str>,
    pub kernel: Option<&'a str>,
    /// RAM the kernel manages (MemTotal): installed RAM minus what firmware and the
    /// kernel reserve at boot.
    pub total: Bytes,
}

// ─── Overview ────────────────────────────────────────────────────────────────────────────

pub struct Overview {
    pub total: Bytes,
    /// Groups that hold any memory, in [`Reclaim::ALL`] order.
    pub groups: Vec<Group>,
    /// How much the kernel's counters overlap, when it's enough to matter.
    pub overcounted: Option<Bytes>,
    /// The gap between free + cache and the kernel's own estimate of what's available.
    pub adjustment: Option<Adjustment>,
    /// The kernel's estimate of what programs could get without swapping (MemAvailable).
    pub available: Bytes,
    /// How much the ZFS ARC could give back, if ZFS is present.
    pub arc_shrink: Option<Bytes>,
    /// "used" as procps `free` computed it before 4.0.
    pub classic_used: Bytes,
}

impl Overview {
    pub fn total_available(&self) -> Option<Bytes> {
        self.arc_shrink.map(|shrink| self.available + shrink)
    }
}

pub struct Group {
    pub reclaim: Reclaim,
    pub slices: NonEmpty<Slice>,
}

impl Group {
    pub fn bytes(&self) -> Bytes {
        self.slices.iter().map(|slice| slice.bytes).sum()
    }
}

/// MemAvailable starts from free memory and droppable cache, then subtracts the reserves
/// the kernel keeps for itself (and occasionally adds memory it can reclaim elsewhere).
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjustment {
    HeldBack(Bytes),
    Extra(Bytes),
}

fn overview(snapshot: &Snapshot, breakdown: &Breakdown) -> Overview {
    let mem = &snapshot.meminfo;
    let groups: Vec<Group> = Reclaim::ALL
        .into_iter()
        .filter_map(|reclaim| {
            let slices = breakdown.of_reclaim(reclaim).filter(|s| s.bytes > Bytes::ZERO);
            NonEmpty::from_vec(slices.collect()).map(|slices| Group { reclaim, slices })
        })
        .collect();
    let free_and_cache: Bytes = groups
        .iter()
        .filter(|group| matches!(group.reclaim, Reclaim::Free | Reclaim::Cache))
        .map(Group::bytes)
        .sum();
    let adjustment = match mem.available.cmp(&free_and_cache) {
        _ if mem.available.0.abs_diff(free_and_cache.0) < NOTICEABLE.0 => None,
        std::cmp::Ordering::Less => {
            Some(Adjustment::HeldBack(free_and_cache.saturating_sub(mem.available)))
        }
        _ => Some(Adjustment::Extra(mem.available.saturating_sub(free_and_cache))),
    };
    let overcounted = match breakdown.remainder {
        Remainder::Overcounted(extra) if extra >= Bytes(64 << 20) => Some(extra),
        _ => None,
    };
    Overview {
        total: breakdown.total,
        groups,
        overcounted,
        adjustment,
        available: mem.available,
        arc_shrink: snapshot.arc.available().map(ArcStats::shrinkable),
        classic_used: mem.classic_used(),
    }
}

// ─── Programs ────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Programs<'a> {
    /// Largest first.
    pub rows: Vec<ProgramRow<'a>>,
    /// The rows beyond the limit, summed.
    pub hidden: Option<Hidden>,
    pub total: Total,
    /// Processes of other users, whose memory only root may read.
    pub unreadable: usize,
}

#[derive(serde::Serialize)]
pub struct ProgramRow<'a> {
    /// The process itself, or a program's root process.
    pub pid: u32,
    pub name: &'a str,
    #[serde(flatten)]
    pub stands_for: StandsFor<'a>,
    pub mem: Rollup,
}

/// What a row of the programs table describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StandsFor<'a> {
    /// One process, and its command line if it has one.
    Process { command: Option<&'a str> },
    /// All of a program's processes.
    Program { processes: Members },
}

/// How many processes a program row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Members {
    One,
    /// Two or more.
    Many(usize),
}

impl Members {
    fn count(processes: usize) -> Self {
        if processes > 1 { Self::Many(processes) } else { Self::One }
    }

    pub fn processes(self) -> usize {
        match self {
            Self::One => 1,
            Self::Many(count) => count,
        }
    }
}

/// In JSON, the number of processes.
impl serde::Serialize for Members {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.processes() as u64)
    }
}

#[derive(serde::Serialize)]
pub struct Hidden {
    pub rows: usize,
    pub mem: Rollup,
}

#[derive(serde::Serialize)]
pub struct Total {
    pub processes: usize,
    pub mem: Rollup,
}

fn programs<'a>(scan: &'a ProcessScan, settings: &Settings) -> Programs<'a> {
    let process_row = |process: &'a Process| ProgramRow {
        pid: process.pid,
        name: &process.name,
        stands_for: StandsFor::Process { command: process.command.as_deref() },
        mem: process.mem,
    };
    let rows: Vec<ProgramRow> = match settings.grouping {
        Grouping::PerProcess => scan.processes.iter().map(process_row).collect(),
        Grouping::ByProgram => procs::by_program(&scan.processes)
            .into_iter()
            .map(|group| ProgramRow {
                pid: group.root().pid,
                name: group.name(),
                stands_for: StandsFor::Program { processes: Members::count(group.processes()) },
                mem: group.mem(),
            })
            .collect(),
    };
    let (rows, rest) = settings.programs.split(sorted_by_key(rows, |row| Reverse(row.mem.pss)));
    Programs {
        rows,
        hidden: (!rest.is_empty())
            .then(|| Hidden { rows: rest.len(), mem: rest.iter().map(|row| row.mem).sum() }),
        total: Total {
            processes: scan.processes.len(),
            mem: scan.processes.iter().map(|process| process.mem).sum(),
        },
        unreadable: scan.unreadable,
    }
}

// ─── Kernel ──────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Kernel<'a> {
    pub slab_unreclaimable: Bytes,
    pub slab_reclaimable: Bytes,
    pub page_tables: Bytes,
    pub stacks: Bytes,
    pub percpu: Bytes,
    /// Overlaps the other counters, so it isn't part of any total.
    pub vmalloc: Bytes,
    pub slabs: Source<Slabs<'a>>,
}

#[derive(serde::Serialize)]
pub struct Slabs<'a> {
    /// Largest first.
    pub largest: Vec<&'a SlabCache>,
    pub count: usize,
    pub footprint: Bytes,
    pub in_use: Bytes,
    /// Empty space inside partly filled slabs.
    pub wasted: Bytes,
    /// The part of `wasted` in caches the kernel can't reclaim, if the allocator says.
    pub wasted_unreclaimable: Option<Bytes>,
}

fn kernel(snapshot: &Snapshot, limit: Limit) -> Kernel<'_> {
    let mem = &snapshot.meminfo;
    Kernel {
        slab_unreclaimable: mem.slab_unreclaimable,
        slab_reclaimable: mem.slab_reclaimable,
        page_tables: mem.page_tables + mem.secondary_page_tables,
        stacks: mem.kernel_stack,
        percpu: mem.percpu,
        vmalloc: mem.vmalloc_used,
        slabs: snapshot.slabs.as_ref().map(|caches| slabs(caches, limit)),
    }
}

fn slabs(caches: &[SlabCache], limit: Limit) -> Slabs<'_> {
    let by_size = sorted_by_key(caches.iter().collect(), |cache| Reverse(cache.footprint));
    let known = caches.iter().any(|c| c.reclaimability != Reclaimability::Unknown);
    let unreclaimable = caches.iter().filter(|c| c.reclaimability == Reclaimability::Unreclaimable);
    Slabs {
        largest: limit.split(by_size).0,
        count: caches.len(),
        footprint: caches.iter().map(|c| c.footprint).sum(),
        in_use: caches.iter().map(SlabCache::in_use).sum(),
        wasted: caches.iter().map(SlabCache::wasted).sum(),
        wasted_unreclaimable: known.then(|| unreclaimable.map(SlabCache::wasted).sum()),
    }
}

// ─── File cache and shared memory ────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Cache<'a> {
    pub file: Bytes,
    pub active: Bytes,
    pub inactive: Bytes,
    pub mapped: Bytes,
    /// Dirty and under writeback: must reach disk before it can be dropped.
    pub unwritten: Bytes,
    pub shared: Bytes,
    /// The largest tmpfs mounts, largest first.
    pub mounts: Vec<&'a TmpfsMount>,
    /// Shared memory outside any visible tmpfs (SysV/POSIX shm, memfd, GPU buffers).
    pub elsewhere: Bytes,
}

fn cache<'a>(snapshot: &'a Snapshot, breakdown: &Breakdown) -> Cache<'a> {
    let mem = &snapshot.meminfo;
    let in_mounts: Bytes = snapshot.tmpfs.iter().map(|mount| mount.used).sum();
    Cache {
        file: breakdown.bytes(Category::FileCache),
        active: mem.active_file,
        inactive: mem.inactive_file,
        mapped: mem.mapped,
        unwritten: mem.dirty + mem.writeback,
        shared: mem.shmem,
        mounts: snapshot.tmpfs.iter().filter(|mount| mount.used >= NOTICEABLE).take(8).collect(),
        elsewhere: mem.shmem.saturating_sub(in_mounts),
    }
}

// ─── Swap ────────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Swap<'a> {
    pub used: Bytes,
    pub total: Bytes,
    pub devices: Source<&'a Vec<SwapDevice>>,
    /// zram devices holding data.
    pub zram: Vec<&'a Zram>,
    /// Pages kept both in RAM and in swap.
    pub cache: Bytes,
}

fn swap(snapshot: &Snapshot) -> Option<Swap<'_>> {
    let mem = &snapshot.meminfo;
    let zram: Vec<&Zram> = snapshot.zram.iter().filter(|z| z.original > Bytes::ZERO).collect();
    (mem.swap_total > Bytes::ZERO || !zram.is_empty()).then(|| Swap {
        used: mem.swap_total.saturating_sub(mem.swap_free),
        total: mem.swap_total,
        devices: snapshot.swaps.as_ref(),
        zram,
        cache: mem.swap_cached,
    })
}

// ─── Fragmentation ───────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Fragmentation {
    /// Free memory by the size of the contiguous block it sits in, largest blocks first.
    #[serde(serialize_with = "as_map")]
    pub free: [(BlockSize, Bytes); 3],
    pub verdict: Verdict,
}

impl Fragmentation {
    pub fn total(&self) -> Bytes {
        self.free.iter().map(|(_, bytes)| *bytes).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// At least half of free memory is in large blocks.
    Healthy,
    /// A fifth to a half.
    Fragmenting,
    /// Less than a fifth: huge pages need the kernel to compact memory first.
    Fragmented,
}

/// Key/value pairs as a JSON object.
fn as_map<S: serde::Serializer, K: serde::Serialize, V: serde::Serialize>(
    pairs: &[(K, V)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(pairs.iter().map(|(key, value)| (key, value)))
}

fn fragmentation(snapshot: &Snapshot) -> Source<Fragmentation> {
    let measured = snapshot.buddy.as_ref().map(|zones| {
        let free = buddy::free_by_block_size(zones, snapshot.page_size);
        let total: Bytes = free.iter().map(|(_, bytes)| *bytes).sum();
        let large: Bytes =
            free.iter().filter(|(size, _)| *size == BlockSize::Large).map(|(_, b)| *b).sum();
        let verdict = match large.fraction_of(total) {
            f if f >= 0.5 => Verdict::Healthy,
            f if f >= 0.2 => Verdict::Fragmenting,
            _ => Verdict::Fragmented,
        };
        Fragmentation { free, verdict }
    });
    // With hardly any free memory, its shape says nothing.
    match measured {
        Source::Available(f) if f.total() < Bytes(64 << 20) => Source::Absent,
        other => other,
    }
}

// ─── Unexplained ─────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct Unexplained {
    pub amount: Bytes,
    /// vmalloc'd memory not counted as kernel stacks: a possible hiding place.
    pub vmalloc: Bytes,
    /// Memory handed back to a hypervisor.
    pub balloon: Bytes,
}

fn unexplained(snapshot: &Snapshot, breakdown: &Breakdown) -> Option<Unexplained> {
    let mem = &snapshot.meminfo;
    let amount = breakdown.unexplained();
    (amount >= NOTICEABLE).then(|| Unexplained {
        amount,
        vmalloc: mem.vmalloc_used.saturating_sub(mem.kernel_stack),
        balloon: mem.balloon,
    })
}
