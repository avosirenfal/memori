//! /proc/slabinfo: the kernel's object caches, as `slabtop` shows them.

use std::fs;

use anyhow::{Context, ensure};

use crate::units::Bytes;

pub const SLABINFO: &str = "/proc/slabinfo";

#[derive(Debug, Clone)]
pub struct SlabCache {
    pub name: String,
    pub active_objects: u64,
    pub object_size: Bytes,
    /// Memory the cache occupies: all of its slabs, including empty slots (slabtop's CACHE SIZE).
    pub footprint: Bytes,
    pub reclaimability: Reclaimability,
}

/// Whether the kernel counts a cache as reclaimable (SReclaimable) or not (SUnreclaim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reclaimability {
    Reclaimable,
    Unreclaimable,
    /// The allocator doesn't say (SLAB rather than SLUB, or sysfs unavailable).
    Unknown,
}

impl SlabCache {
    pub fn in_use(&self) -> Bytes {
        self.object_size.times(self.active_objects)
    }

    /// Slab memory that holds no live object: free slots in partly filled slabs, plus padding.
    pub fn wasted(&self) -> Bytes {
        self.footprint.saturating_sub(self.in_use())
    }
}

/// Parses /proc/slabinfo; `reclaimability` looks a cache up by name.
pub fn parse(
    text: &str,
    page_size: Bytes,
    reclaimability: impl Fn(&str) -> Reclaimability,
) -> anyhow::Result<Vec<SlabCache>> {
    let mut lines = text.lines();
    let version = lines.next().context("empty file")?;
    ensure!(version.starts_with("slabinfo - version: 2."), "unknown format: {version}");
    lines
        .filter(|line| !line.starts_with('#'))
        .map(|line| parse_line(line, page_size, &reclaimability))
        .collect()
}

/// `name active_objs num_objs objsize objperslab pagesperslab : tunables .. : slabdata active_slabs num_slabs sharedavail`
fn parse_line(
    line: &str,
    page_size: Bytes,
    reclaimability: &impl Fn(&str) -> Reclaimability,
) -> anyhow::Result<SlabCache> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    ensure!(fields.len() >= 15, "short line: {line}");
    let number = |index: usize| -> anyhow::Result<u64> {
        fields[index].parse().with_context(|| format!("bad number in: {line}"))
    };
    let (pages_per_slab, slabs) = (number(5)?, number(14)?);
    Ok(SlabCache {
        name: fields[0].to_owned(),
        active_objects: number(1)?,
        object_size: Bytes(number(3)?),
        footprint: page_size.times(pages_per_slab * slabs),
        reclaimability: reclaimability(fields[0]),
    })
}

/// SLUB publishes each cache's reclaimability in sysfs.
pub fn reclaimability_from_sysfs(name: &str) -> Reclaimability {
    match fs::read_to_string(format!("/sys/kernel/slab/{name}/reclaim_account")).as_deref() {
        Ok("1\n") => Reclaimability::Reclaimable,
        Ok("0\n") => Reclaimability::Unreclaimable,
        _ => Reclaimability::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
slabinfo - version: 2.1
# name            <active_objs> <num_objs> <objsize> <objperslab> <pagesperslab> : tunables <limit> <batchcount> <sharedfactor> : slabdata <active_slabs> <num_slabs> <sharedavail>
dentry             1000   2000    192   21    1 : tunables    0    0    0 : slabdata     95     100      0
kmalloc-8k           10     12   8192    4    8 : tunables    0    0    0 : slabdata      3       3      0
";

    #[test]
    fn computes_footprint_and_waste_like_slabtop() {
        let reclaimability = |name: &str| match name {
            "dentry" => Reclaimability::Reclaimable,
            _ => Reclaimability::Unreclaimable,
        };
        let caches = parse(SAMPLE, Bytes(4096), reclaimability).unwrap();
        let dentry = &caches[0];
        assert_eq!(dentry.footprint, Bytes(100 * 4096));
        assert_eq!(dentry.in_use(), Bytes(1000 * 192));
        assert_eq!(dentry.wasted(), Bytes(100 * 4096 - 1000 * 192));
        assert_eq!(dentry.reclaimability, Reclaimability::Reclaimable);
        assert_eq!(caches[1].footprint, Bytes(3 * 8 * 4096));
    }
}
