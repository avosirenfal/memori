//! Everything read from the system, gathered once before any analysis.

use std::fs;

use anyhow::Context;

use crate::buddy::{self, Zone};
use crate::gpu::{self, GpuGtt};
use crate::meminfo::{self, MemInfo};
use crate::procs::{self, ProcessScan};
use crate::slab::{self, SlabCache};
use crate::source::Source;
use crate::swap::{self, SwapDevice, Zram};
use crate::tmpfs::{self, TmpfsMount};
use crate::units::Bytes;
use crate::zfs::{self, ArcStats};

#[derive(Debug)]
pub struct Snapshot {
    pub hostname: String,
    pub kernel: String,
    pub page_size: Bytes,
    pub meminfo: MemInfo,
    pub arc: Source<ArcStats>,
    pub slabs: Source<Vec<SlabCache>>,
    pub processes: ProcessScan,
    pub buddy: Source<Vec<Zone>>,
    pub swaps: Source<Vec<SwapDevice>>,
    pub zram: Vec<Zram>,
    /// RAM held by zsmalloc (zram and zswap storage), from /proc/vmstat.
    pub zsmalloc_pool: Bytes,
    pub tmpfs: Vec<TmpfsMount>,
    pub gpus: Vec<GpuGtt>,
}

impl Snapshot {
    pub fn collect() -> anyhow::Result<Self> {
        let page_size = Bytes(rustix::param::page_size() as u64);
        let meminfo_text = fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?;
        let read_trimmed = |path: &str| {
            fs::read_to_string(path).map_or_else(|_| "?".to_owned(), |s| s.trim().to_owned())
        };
        Ok(Self {
            hostname: read_trimmed("/proc/sys/kernel/hostname"),
            kernel: read_trimmed("/proc/sys/kernel/osrelease"),
            page_size,
            meminfo: meminfo::parse(&meminfo_text).context("parsing /proc/meminfo")?,
            arc: Source::read(zfs::ARCSTATS, zfs::parse),
            slabs: Source::read(slab::SLABINFO, |text| {
                slab::parse(text, page_size, slab::reclaimable_from_sysfs)
            }),
            processes: procs::scan(),
            buddy: Source::read(buddy::BUDDYINFO, buddy::parse),
            swaps: Source::read(swap::SWAPS, swap::parse_swaps),
            zram: swap::read_zram(),
            zsmalloc_pool: swap::read_zsmalloc_pool(page_size),
            tmpfs: tmpfs::read(),
            gpus: gpu::read(),
        })
    }

    /// RAM holding compressed swap. zsmalloc covers zram and most zswap setups; zswap on
    /// another allocator reports its pool in /proc/meminfo instead. Take whichever is larger.
    pub fn compressed_swap(&self) -> Bytes {
        let zram: Bytes = self.zram.iter().map(|z| z.ram_used).sum();
        self.zsmalloc_pool.max(zram + self.meminfo.zswap)
    }

    pub fn gpu_gtt(&self) -> Bytes {
        self.gpus.iter().map(|g| g.used).sum()
    }
}
