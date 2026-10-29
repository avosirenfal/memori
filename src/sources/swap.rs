//! Swap devices (/proc/swaps), zram devices, and the compressed-memory pool.

use std::fs;

use anyhow::Context;

use super::unescape_octal;
use crate::units::Bytes;

pub const SWAPS: &str = "/proc/swaps";

#[derive(Debug, Clone)]
pub struct SwapDevice {
    pub path: String,
    pub kind: SwapKind,
    pub size: Bytes,
    pub used: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapKind {
    /// A compressed RAM disk: swapped pages still occupy RAM, compressed.
    Zram,
    Partition,
    File,
    Other(String),
}

impl SwapKind {
    fn of(path: &str, kind: &str) -> Self {
        match kind {
            _ if path.starts_with("/dev/zram") => Self::Zram,
            "partition" => Self::Partition,
            "file" => Self::File,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// `Filename Type Size Used Priority`, sizes in KiB, after one header line.
pub fn parse_swaps(text: &str) -> anyhow::Result<Vec<SwapDevice>> {
    text.lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let kib = |index: usize| -> anyhow::Result<Bytes> {
                let value: u64 = fields
                    .get(index)
                    .and_then(|field| field.parse().ok())
                    .with_context(|| format!("bad line: {line}"))?;
                Ok(Bytes(value * 1024))
            };
            let path = unescape_octal(fields[0]);
            Ok(SwapDevice {
                kind: SwapKind::of(&path, fields.get(1).copied().unwrap_or_default()),
                path,
                size: kib(2)?,
                used: kib(3)?,
            })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Zram {
    pub name: String,
    /// Data stored in the device, before compression.
    pub original: Bytes,
    /// RAM the device occupies, including allocator overhead.
    pub ram_used: Bytes,
}

pub fn read_zram() -> Vec<Zram> {
    fs::read_dir("/sys/block")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let stats = fs::read_to_string(entry.path().join("mm_stat")).ok()?;
            // orig_data_size compr_data_size mem_used_total mem_limit ...
            let numbers: Vec<u64> =
                stats.split_whitespace().take(3).filter_map(|n| n.parse().ok()).collect();
            match numbers[..] {
                [original, _, ram_used] if name.starts_with("zram") => {
                    Some(Zram { name, original: Bytes(original), ram_used: Bytes(ram_used) })
                }
                _ => None,
            }
        })
        .collect()
}

/// RAM held by zsmalloc, the allocator behind zram and (usually) zswap.
pub fn read_zsmalloc_pool(page_size: Bytes) -> Bytes {
    fs::read_to_string("/proc/vmstat")
        .ok()
        .and_then(|vmstat| {
            vmstat.lines().find_map(|line| line.strip_prefix("nr_zspages ")?.trim().parse().ok())
        })
        .map_or(Bytes::ZERO, |pages| page_size.times(pages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swaps() {
        let text =
            "Filename\tType\tSize\tUsed\tPriority\n/dev/zram0  partition  8388604  1024  100\n";
        let devices = parse_swaps(text).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].path, "/dev/zram0");
        assert_eq!(devices[0].kind, SwapKind::Zram);
        assert_eq!(devices[0].used, Bytes(1024 * 1024));
    }
}
