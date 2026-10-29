//! The report's sections. Each section's title, command-line flag and description are
//! defined here once; the CLI and the renderer both read them from [`SectionKind::spec`].

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionKind {
    Overview,
    Programs,
    Zfs,
    Kernel,
    Cache,
    Swap,
    Fragmentation,
    Unexplained,
}

pub struct SectionSpec {
    /// The heading printed above the section.
    pub title: &'static str,
    /// The long flag that selects the section, without dashes.
    pub flag: &'static str,
    pub short: char,
    /// One line for `--help`.
    pub about: &'static str,
}

impl SectionKind {
    /// Every section, in report order.
    pub const ALL: [Self; 8] = [
        Self::Overview,
        Self::Programs,
        Self::Zfs,
        Self::Kernel,
        Self::Cache,
        Self::Swap,
        Self::Fragmentation,
        Self::Unexplained,
    ];

    /// What prints when no section is chosen.
    pub const SUMMARY: [Self; 2] = [Self::Overview, Self::Programs];

    pub const fn spec(self) -> SectionSpec {
        let (title, flag, short, about) = match self {
            Self::Overview => {
                ("OVERVIEW", "overview", 'o', "Where RAM is, and how much is available")
            }
            Self::Programs => ("PROGRAMS", "programs", 'p', "Programs and their memory"),
            Self::Zfs => ("ZFS ARC", "zfs", 'z', "ZFS's disk cache"),
            Self::Kernel => ("KERNEL", "kernel", 'k', "Kernel memory and slab caches"),
            Self::Cache => {
                ("FILE CACHE & SHARED MEMORY", "cache", 'c', "File cache and shared memory")
            }
            Self::Swap => ("SWAP", "swap", 's', "Swap devices and zram"),
            Self::Fragmentation => {
                ("FREE MEMORY FRAGMENTATION", "fragmentation", 'f', "Free memory fragmentation")
            }
            Self::Unexplained => {
                ("UNEXPLAINED MEMORY", "unexplained", 'u', "Memory no kernel counter accounts for")
            }
        };
        SectionSpec { title, flag, short, about }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_section_once_in_order() {
        let sorted = {
            let mut all = SectionKind::ALL.to_vec();
            all.sort();
            all.dedup();
            all
        };
        assert_eq!(sorted, SectionKind::ALL);
    }

    #[test]
    fn flags_are_unique() {
        let shorts: std::collections::HashSet<char> =
            SectionKind::ALL.iter().map(|kind| kind.spec().short).collect();
        assert_eq!(shorts.len(), SectionKind::ALL.len());
    }
}
