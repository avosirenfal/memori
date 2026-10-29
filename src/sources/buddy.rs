//! /proc/buddyinfo: how free memory is split into contiguous blocks.

use anyhow::Context;

use crate::units::Bytes;

pub const BUDDYINFO: &str = "/proc/buddyinfo";

/// One memory zone's free-block counts; index `n` counts free blocks of 2ⁿ pages.
#[derive(Debug, Clone)]
pub struct Zone {
    pub free_blocks: Vec<u64>,
}

pub fn parse(text: &str) -> anyhow::Result<Vec<Zone>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_line(line).with_context(|| format!("bad line: {line}")))
        .collect()
}

/// `Node 0, zone   Normal   43   85   11 ...`
fn parse_line(line: &str) -> Option<Zone> {
    let (_node, rest) = line.strip_prefix("Node")?.split_once(',')?;
    let mut fields = rest.split_whitespace().skip_while(|&field| field != "zone").skip(2);
    let free_blocks = fields.by_ref().map(str::parse).collect::<Result<Vec<_>, _>>().ok()?;
    (!free_blocks.is_empty()).then_some(Zone { free_blocks })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockSize {
    Small,
    Medium,
    Large,
}

impl BlockSize {
    pub const ALL: [Self; 3] = [Self::Large, Self::Medium, Self::Small];
    /// Huge pages (and many driver allocations) need blocks at least this big.
    pub const LARGE: Bytes = Bytes(2 << 20);
    pub const MEDIUM: Bytes = Bytes(64 << 10);

    fn of(block: Bytes) -> Self {
        match block {
            b if b >= Self::LARGE => Self::Large,
            b if b >= Self::MEDIUM => Self::Medium,
            _ => Self::Small,
        }
    }
}

/// Free memory across all zones, split by the size of the contiguous block it sits in.
pub fn free_by_block_size(zones: &[Zone], page_size: Bytes) -> [(BlockSize, Bytes); 3] {
    let blocks: Vec<(BlockSize, Bytes)> = zones
        .iter()
        .flat_map(|zone| zone.free_blocks.iter().enumerate())
        .map(|(order, &count)| {
            let block = page_size.times(1 << order);
            (BlockSize::of(block), block.times(count))
        })
        .collect();
    BlockSize::ALL.map(|size| {
        let total = blocks.iter().filter(|(s, _)| *s == size).map(|(_, bytes)| *bytes).sum();
        (size, total)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_free_blocks() {
        let text = "Node 0, zone      DMA      1      0      0      0      0      0      0      0      0      1      0\n";
        let zones = parse(text).unwrap();
        assert_eq!(zones[0].free_blocks.len(), 11);
        let split = free_by_block_size(&zones, Bytes(4096));
        assert_eq!(split[0], (BlockSize::Large, Bytes(2 << 20)));
        assert_eq!(split[1], (BlockSize::Medium, Bytes::ZERO));
        assert_eq!(split[2], (BlockSize::Small, Bytes(4096)));
    }
}
