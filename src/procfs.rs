//! Small helpers shared by the parsers of kernel text files.

use std::collections::HashMap;

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

/// Returns `items` sorted by `key` (stable).
pub fn sorted_by_key<T, K: Ord>(items: Vec<T>, key: impl FnMut(&T) -> K) -> Vec<T> {
    let mut items = items;
    items.sort_by_key(key);
    items
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
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
