//! What happened when we tried to read one optional data source.

use std::{fs, io, path::Path};

#[derive(Debug)]
pub enum Source<T> {
    Available(T),
    /// This system doesn't have the feature (for example, ZFS isn't loaded).
    Absent,
    /// The kernel only shows this file to root.
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
}
