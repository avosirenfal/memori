//! Splits total RAM into non-overlapping categories, so every byte is accounted for once.
//!
//! The identity behind it: MemTotal = MemFree + Buffers + Cached + SReclaimable + "used",
//! where Cached includes Shmem, and "used" is the sum of the kernel's other counters plus
//! whatever no counter tracks.

use crate::snapshot::Snapshot;
use crate::units::Bytes;
use crate::zfs::ArcStats;

/// How hard it is to get memory back from a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reclaim {
    /// Only freed by closing programs (or by pushing it out to swap).
    InUse,
    /// Handed back by its owner when the kernel asks.
    Shrinkable,
    /// Dropped by the kernel on demand.
    Cache,
    Free,
}

impl Reclaim {
    pub const ALL: [Self; 4] = [Self::InUse, Self::Shrinkable, Self::Cache, Self::Free];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Programs,
    SharedMemory,
    KernelSlab,
    PageTables,
    StacksAndPercpu,
    CompressedSwap,
    Gpu,
    HugePages,
    Unexplained,
    ZfsArc,
    FileCache,
    KernelCaches,
    Free,
}

impl Category {
    pub const fn reclaim(self) -> Reclaim {
        match self {
            Self::Programs
            | Self::SharedMemory
            | Self::KernelSlab
            | Self::PageTables
            | Self::StacksAndPercpu
            | Self::CompressedSwap
            | Self::Gpu
            | Self::HugePages
            | Self::Unexplained => Reclaim::InUse,
            Self::ZfsArc => Reclaim::Shrinkable,
            Self::FileCache | Self::KernelCaches => Reclaim::Cache,
            Self::Free => Reclaim::Free,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    pub category: Category,
    pub bytes: Bytes,
}

/// What's left after subtracting every counter from the total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remainder {
    /// RAM no counter accounts for (drivers allocating pages directly, for example).
    Unexplained(Bytes),
    /// The counters add up to more than the total, because some of them overlap.
    Overcounted(Bytes),
}

#[derive(Debug, Clone)]
pub struct Breakdown {
    pub total: Bytes,
    /// Every category except `Unexplained`, which lives in `remainder`.
    known: Vec<Slice>,
    pub remainder: Remainder,
}

impl Breakdown {
    pub fn of(snapshot: &Snapshot) -> Self {
        let mem = &snapshot.meminfo;
        let arc = snapshot.arc.available();
        let known: Vec<Slice> = [
            (Category::Programs, mem.anon_pages),
            (Category::SharedMemory, mem.shmem),
            (Category::KernelSlab, mem.slab_unreclaimable),
            (Category::PageTables, mem.page_tables + mem.secondary_page_tables),
            (Category::StacksAndPercpu, mem.kernel_stack + mem.percpu),
            (Category::CompressedSwap, snapshot.compressed_swap()),
            (Category::Gpu, snapshot.gpu_gtt()),
            (Category::HugePages, mem.hugetlb),
            (Category::ZfsArc, arc.map_or(Bytes::ZERO, ArcStats::outside_slab)),
            (Category::FileCache, mem.cached.saturating_sub(mem.shmem) + mem.buffers),
            (Category::KernelCaches, mem.slab_reclaimable),
            (Category::Free, mem.free),
        ]
        .map(|(category, bytes)| Slice { category, bytes })
        .to_vec();
        let accounted: Bytes = known.iter().map(|slice| slice.bytes).sum();
        let remainder = if accounted <= mem.total {
            Remainder::Unexplained(mem.total.saturating_sub(accounted))
        } else {
            Remainder::Overcounted(accounted.saturating_sub(mem.total))
        };
        Self { total: mem.total, known, remainder }
    }

    pub fn unexplained(&self) -> Bytes {
        match self.remainder {
            Remainder::Unexplained(bytes) => bytes,
            Remainder::Overcounted(_) => Bytes::ZERO,
        }
    }

    /// All categories, including the unexplained remainder.
    pub fn slices(&self) -> Vec<Slice> {
        let unexplained = Slice { category: Category::Unexplained, bytes: self.unexplained() };
        self.known.iter().copied().chain([unexplained]).collect()
    }

    pub fn of_reclaim(&self, reclaim: Reclaim) -> Vec<Slice> {
        self.slices().into_iter().filter(|s| s.category.reclaim() == reclaim).collect()
    }

    pub fn bytes(&self, category: Category) -> Bytes {
        self.slices().iter().filter(|s| s.category == category).map(|s| s.bytes).sum()
    }
}
