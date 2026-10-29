//! Readers and parsers for the kernel's memory files. Each parser is a pure function of
//! the file's text; reading the file is kept separate so parsers can be tested.

pub mod buddy;
pub mod gpu;
pub mod meminfo;
pub mod procs;
pub mod slab;
pub mod swap;
pub mod tmpfs;
pub mod zfs;

use std::collections::HashMap;
use std::{fs, io, path::Path};

/// What happened when we tried to read one optional data source.
#[derive(Debug, Clone)]
pub enum Source<T> {
    Available(T),
    /// This system doesn't have the feature (for example, ZFS isn't loaded).
    Absent,
    /// The kernel only shows this to root.
    NeedsRoot,
    /// The file exists but couldn't be read or understood.
    Broken(String),
}

impl<T> Source<T> {
    pub fn read(path: impl AsRef<Path>, parse: impl FnOnce(&str) -> anyhow::Result<T>) -> Self {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(text) => parse(&text).map_or_else(
                |error| Self::Broken(format!("{}: {error:#}", path.display())),
                Self::Available,
            ),
            Err(error) => match error.kind() {
                io::ErrorKind::NotFound => Self::Absent,
                io::ErrorKind::PermissionDenied => Self::NeedsRoot,
                _ => Self::Broken(format!("{}: {error}", path.display())),
            },
        }
    }

    pub fn available(&self) -> Option<&T> {
        match self {
            Self::Available(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_ref(&self) -> Source<&T> {
        self.map_ref(|value| value)
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Source<U> {
        match self {
            Self::Available(value) => Source::Available(f(value)),
            Self::Absent => Source::Absent,
            Self::NeedsRoot => Source::NeedsRoot,
            Self::Broken(reason) => Source::Broken(reason),
        }
    }

    fn map_ref<'a, U>(&'a self, f: impl FnOnce(&'a T) -> U) -> Source<U> {
        match self {
            Self::Available(value) => Source::Available(f(value)),
            Self::Absent => Source::Absent,
            Self::NeedsRoot => Source::NeedsRoot,
            Self::Broken(reason) => Source::Broken(reason.clone()),
        }
    }
}

/// In JSON: the value itself, `null` when the system doesn't have it, or
/// `{"error": "..."}` when it couldn't be read.
impl<T: serde::Serialize> serde::Serialize for Source<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct Error<'a> {
            error: &'a str,
        }
        match self {
            Self::Available(value) => value.serialize(serializer),
            Self::Absent => serializer.serialize_none(),
            Self::NeedsRoot => Error { error: "needs root" }.serialize(serializer),
            Self::Broken(reason) => Error { error: reason }.serialize(serializer),
        }
    }
}

/// Parses lines like `MemTotal:  16477352 kB` or `HugePages_Total:  0`, the format of
/// /proc/meminfo and /proc/<pid>/smaps_rollup. Values with a `kB` suffix become bytes;
/// bare values (counts) are kept as they are.
pub fn parse_colon_kv(text: &str) -> HashMap<&str, u64> {
    text.lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let mut parts = rest.split_whitespace();
            let value: u64 = parts.next()?.parse().ok()?;
            let scale = if parts.next() == Some("kB") { 1024 } else { 1 };
            Some((key.trim(), value * scale))
        })
        .collect()
}

/// Undoes the octal escaping /proc/mounts and /proc/swaps apply to paths (`\040` is a space).
pub fn unescape_octal(path: &str) -> String {
    fn decode(bytes: &[u8]) -> Vec<u8> {
        match bytes {
            [] => Vec::new(),
            [b'\\', a @ b'0'..=b'3', b @ b'0'..=b'7', c @ b'0'..=b'7', rest @ ..] => {
                let byte = (a - b'0') * 64 + (b - b'0') * 8 + (c - b'0');
                [vec![byte], decode(rest)].concat()
            }
            [first, rest @ ..] => [vec![*first], decode(rest)].concat(),
        }
    }
    String::from_utf8_lossy(&decode(path.as_bytes())).into_owned()
}

/// Declares a struct whose fields are all [`Bytes`](crate::units::Bytes), filled from a
/// key/value map like the one [`parse_colon_kv`] returns (absent keys read as zero).
/// The struct also gets field-wise `+` and `Sum`, so per-process figures can be totalled.
macro_rules! byte_fields {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$field_meta:meta])* $field:ident <- $key:literal ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ::serde::Serialize)]
        $vis struct $name {
            $( $(#[$field_meta])* pub $field: $crate::units::Bytes, )*
        }

        impl $name {
            pub fn from_kv(kv: &::std::collections::HashMap<&str, u64>) -> Self {
                Self {
                    $( $field: $crate::units::Bytes(kv.get($key).copied().unwrap_or(0)), )*
                }
            }
        }

        impl ::std::ops::Add for $name {
            type Output = Self;
            fn add(self, other: Self) -> Self {
                Self { $( $field: self.$field + other.$field, )* }
            }
        }

        impl ::std::iter::Sum for $name {
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                iter.fold(Self::default(), ::std::ops::Add::add)
            }
        }
    };
}
pub(crate) use byte_fields;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kb_and_counts() {
        let kv = parse_colon_kv("MemTotal:  2 kB\nHugePages_Total:  3\nbogus line\n");
        assert_eq!(kv["MemTotal"], 2048);
        assert_eq!(kv["HugePages_Total"], 3);
        assert_eq!(kv.len(), 2);
    }

    #[test]
    fn unescapes_mount_paths() {
        assert_eq!(unescape_octal(r"/mnt/my\040disk"), "/mnt/my disk");
        assert_eq!(unescape_octal(r"/plain"), "/plain");
        assert_eq!(unescape_octal(r"/trailing\04"), r"/trailing\04");
    }
}
