//! A byte count that prints itself the way a person would say it.

use std::fmt;
use std::iter::Sum;
use std::ops::Add;

/// In JSON, a plain number of bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct Bytes(pub u64);

impl Bytes {
    pub const ZERO: Self = Self(0);
    pub const MIB: Self = Self(1 << 20);

    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    pub const fn times(self, factor: u64) -> Self {
        Self(self.0 * factor)
    }

    /// `self / whole` as a fraction in `0.0..`; zero when `whole` is zero.
    pub fn fraction_of(self, whole: Self) -> f64 {
        if whole.0 == 0 { 0.0 } else { self.0 as f64 / whole.0 as f64 }
    }
}

impl Add for Bytes {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

impl Sum for Bytes {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

/// GiB and TiB get one decimal ("12.3 GiB"); smaller units are whole numbers ("512 MiB").
/// Honors width and alignment, so `{:>10}` lines up columns.
impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [(&str, u64, usize); 4] =
            [("TiB", 1 << 40, 1), ("GiB", 1 << 30, 1), ("MiB", 1 << 20, 0), ("KiB", 1 << 10, 0)];
        let text = match UNITS.iter().find(|(_, scale, _)| self.0 >= *scale) {
            Some((unit, scale, decimals)) => {
                format!("{:.*} {unit}", decimals, self.0 as f64 / *scale as f64)
            }
            None => format!("{} B", self.0),
        };
        f.pad(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_human_units() {
        assert_eq!(Bytes(0).to_string(), "0 B");
        assert_eq!(Bytes(1536).to_string(), "2 KiB");
        assert_eq!(Bytes(300 << 20).to_string(), "300 MiB");
        assert_eq!(Bytes(3 << 29).to_string(), "1.5 GiB");
        assert_eq!(format!("{:>9}", Bytes(1 << 30)), "  1.0 GiB");
    }
}
