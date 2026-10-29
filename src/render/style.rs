//! How things look: names, colors and number formats shared by every section.

use anstyle::{AnsiColor, Color, Style};

use crate::analysis::Verdict;
use crate::analysis::breakdown::{Category, Reclaim};
use crate::sources::buddy::BlockSize;
use crate::sources::slab::Reclaimability;
use crate::sources::swap::SwapKind;
use crate::units::Bytes;

pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();
pub const WARN: Style = fg(AnsiColor::Yellow).bold();

pub const fn fg(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(color)))
}

pub fn label(category: Category) -> &'static str {
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

pub fn color(category: Category) -> Style {
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
pub fn glyph(reclaim: Reclaim) -> &'static str {
    match reclaim {
        Reclaim::InUse => "█",
        Reclaim::Shrinkable => "▓",
        Reclaim::Cache => "▒",
        Reclaim::Free => "░",
    }
}

pub fn group_name(reclaim: Reclaim) -> &'static str {
    match reclaim {
        Reclaim::InUse => "In use",
        Reclaim::Shrinkable => "Shrinkable",
        Reclaim::Cache => "Cache",
        Reclaim::Free => "Free",
    }
}

pub fn block_size(size: BlockSize) -> &'static str {
    match size {
        BlockSize::Large => "Free in blocks of 2 MiB or more",
        BlockSize::Medium => "Free in blocks of 64 KiB up to 2 MiB",
        BlockSize::Small => "Free in blocks under 64 KiB",
    }
}

pub fn verdict(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Healthy => "healthy",
        Verdict::Fragmenting => "getting fragmented",
        Verdict::Fragmented => "fragmented: huge pages need compaction",
    }
}

pub fn freeable(reclaimability: Reclaimability) -> &'static str {
    match reclaimability {
        Reclaimability::Reclaimable => "yes",
        Reclaimability::Unreclaimable => "no",
        Reclaimability::Unknown => "?",
    }
}

pub fn swap_kind(kind: &SwapKind) -> &str {
    match kind {
        SwapKind::Zram => "zram",
        SwapKind::Partition => "partition",
        SwapKind::File => "file",
        SwapKind::Other(other) => other,
    }
}

pub fn percent(part: Bytes, whole: Bytes) -> String {
    match 100.0 * part.fraction_of(whole) {
        p if p > 0.0 && p < 1.0 => "<1%".to_owned(),
        p => format!("{p:.0}%"),
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let kept: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn percent_marks_tiny_shares() {
        assert_eq!(percent(Bytes(1), Bytes(1000)), "<1%");
        assert_eq!(percent(Bytes(500), Bytes(1000)), "50%");
        assert_eq!(percent(Bytes(0), Bytes(0)), "0%");
    }
}
