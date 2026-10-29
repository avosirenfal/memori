//! /proc/spl/kstat/zfs/arcstats: the ZFS Adaptive Replacement Cache.

use std::collections::HashMap;

use anyhow::ensure;

use super::byte_fields;
use crate::units::Bytes;

pub const ARCSTATS: &str = "/proc/spl/kstat/zfs/arcstats";

byte_fields! {
    pub struct ArcSizes {
        size <- "size",
        /// Where the ARC is currently trying to be.
        target <- "c",
        min <- "c_min",
        max <- "c_max",
        data <- "data_size",
        metadata <- "metadata_size",
        headers <- "hdr_size",
        l2_headers <- "l2_hdr_size",
        dbufs <- "dbuf_size",
        dnodes <- "dnode_size",
        bonus <- "bonus_size",
        /// Older OpenZFS reports dbufs + dnodes + bonus buffers as one number.
        other <- "other_size",
    }
}

#[derive(Debug, Clone)]
pub struct ArcStats {
    pub sizes: ArcSizes,
    pub hits: u64,
    pub misses: u64,
}

impl ArcStats {
    /// ZFS's bookkeeping structures (buffer headers, dnodes, dbufs). These are allocated from
    /// ordinary kernel slab caches, so the kernel's slab counters already include them.
    pub fn bookkeeping(&self) -> Bytes {
        let s = &self.sizes;
        let split = s.dbufs + s.dnodes + s.bonus;
        let other = if split > Bytes::ZERO { split } else { s.other };
        s.headers + s.l2_headers + other
    }

    /// The part of the ARC not already counted as kernel slab: cached file data and metadata.
    pub fn outside_slab(&self) -> Bytes {
        self.sizes.size.saturating_sub(self.bookkeeping())
    }

    /// How much the ARC could hand back if it shrank all the way to its minimum.
    pub fn shrinkable(&self) -> Bytes {
        self.sizes.size.saturating_sub(self.sizes.min)
    }

    pub fn hit_ratio(&self) -> Option<f64> {
        let lookups = self.hits + self.misses;
        (lookups > 0).then(|| self.hits as f64 / lookups as f64)
    }
}

/// The file is a two-line header followed by `name type value` rows.
pub fn parse(text: &str) -> anyhow::Result<ArcStats> {
    let kv: HashMap<&str, u64> = text
        .lines()
        .skip(2)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let value = fields.nth(1)?.parse().ok()?;
            Some((name, value))
        })
        .collect();
    ensure!(kv.contains_key("size"), "no ARC size");
    Ok(ArcStats {
        sizes: ArcSizes::from_kv(&kv),
        hits: kv.get("hits").copied().unwrap_or(0),
        misses: kv.get("misses").copied().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
13 1 0x01 147 39984 4204356581 1063419935539
name                            type data
hits                            4    90
misses                          4    10
size                            4    1000
c                               4    1200
c_min                           4    100
c_max                           4    2000
data_size                       4    700
metadata_size                   4    200
hdr_size                        4    40
dbuf_size                       4    30
dnode_size                      4    20
bonus_size                      4    10
other_size                      4    60
";

    #[test]
    fn splits_bookkeeping_from_cached_data() {
        let arc = parse(SAMPLE).unwrap();
        assert_eq!(arc.sizes.size, Bytes(1000));
        assert_eq!(arc.bookkeeping(), Bytes(100));
        assert_eq!(arc.outside_slab(), Bytes(900));
        assert_eq!(arc.shrinkable(), Bytes(900));
        assert_eq!(arc.hit_ratio(), Some(0.9));
    }
}
