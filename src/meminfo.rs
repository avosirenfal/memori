//! /proc/meminfo: the kernel's own system-wide memory counters.

use anyhow::ensure;

use crate::procfs::{byte_fields, parse_colon_kv};
use crate::units::Bytes;

byte_fields! {
    /// The /proc/meminfo lines this tool reasons about.
    pub struct MemInfo {
        total <- "MemTotal",
        free <- "MemFree",
        available <- "MemAvailable",
        buffers <- "Buffers",
        /// Page cache, excluding buffers and swap cache but *including* `shmem`.
        cached <- "Cached",
        swap_cached <- "SwapCached",
        active_file <- "Active(file)",
        inactive_file <- "Inactive(file)",
        swap_total <- "SwapTotal",
        swap_free <- "SwapFree",
        zswap <- "Zswap",
        dirty <- "Dirty",
        writeback <- "Writeback",
        anon_pages <- "AnonPages",
        mapped <- "Mapped",
        shmem <- "Shmem",
        slab_reclaimable <- "SReclaimable",
        slab_unreclaimable <- "SUnreclaim",
        kernel_stack <- "KernelStack",
        page_tables <- "PageTables",
        secondary_page_tables <- "SecPageTables",
        vmalloc_used <- "VmallocUsed",
        percpu <- "Percpu",
        hugetlb <- "Hugetlb",
        balloon <- "Balloon",
    }
}

impl MemInfo {
    /// "used" in the classic sense of `free` (procps before 4.0):
    /// total − free − buffers − cache − reclaimable slab.
    pub fn classic_used(&self) -> Bytes {
        self.total.saturating_sub(self.free + self.buffers + self.cached + self.slab_reclaimable)
    }
}

pub fn parse(text: &str) -> anyhow::Result<MemInfo> {
    let kv = parse_colon_kv(text);
    let info = MemInfo::from_kv(&kv);
    ensure!(info.total > Bytes::ZERO, "no MemTotal line");
    // Kernels before 4.16 have no Hugetlb line; derive it from the default-size pool.
    let hugetlb = match kv.get("Hugetlb") {
        Some(_) => info.hugetlb,
        None => Bytes(kv.get("HugePages_Total").copied().unwrap_or(0))
            .times(kv.get("Hugepagesize").copied().unwrap_or(0)),
    };
    Ok(MemInfo { hugetlb, ..info })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_derives_hugetlb_on_old_kernels() {
        let info =
            parse("MemTotal: 100 kB\nShmem: 4 kB\nHugePages_Total: 2\nHugepagesize: 2048 kB\n")
                .unwrap();
        assert_eq!(info.total, Bytes(100 * 1024));
        assert_eq!(info.shmem, Bytes(4 * 1024));
        assert_eq!(info.hugetlb, Bytes(2 * 2048 * 1024));
        assert_eq!(info.percpu, Bytes::ZERO);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("nonsense").is_err());
    }
}
