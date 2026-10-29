//! tmpfs mounts: files that live only in RAM (counted as Shmem).

use std::collections::BTreeMap;
use std::fs;

use crate::procfs::{sorted_by_key, unescape_octal};
use crate::units::Bytes;

#[derive(Debug, Clone)]
pub struct TmpfsMount {
    pub path: String,
    pub used: Bytes,
}

/// tmpfs mounts visible to this process, largest first. A tmpfs mounted in several places
/// (bind mounts) is listed once.
pub fn read() -> Vec<TmpfsMount> {
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let by_device = tmpfs_paths(&mounts)
        .into_iter()
        .filter_map(|path| {
            let device = rustix::fs::stat(path.as_str()).ok()?.st_dev;
            let usage = rustix::fs::statvfs(path.as_str()).ok()?;
            let used = Bytes(usage.f_blocks.saturating_sub(usage.f_bfree) * usage.f_frsize);
            Some((device, TmpfsMount { path, used }))
        })
        .fold(BTreeMap::new(), |mut unique, (device, mount)| {
            unique.entry(device).or_insert(mount);
            unique
        });
    sorted_by_key(by_device.into_values().collect(), |mount| std::cmp::Reverse(mount.used))
}

/// Mount points of tmpfs filesystems in /proc/mounts (`device mountpoint fstype ...`).
fn tmpfs_paths(mounts: &str) -> Vec<String> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let mountpoint = fields.nth(1)?;
            (fields.next()? == "tmpfs").then(|| unescape_octal(mountpoint))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tmpfs_mounts() {
        let mounts = "proc /proc proc rw 0 0\ntmpfs /dev/shm tmpfs rw 0 0\ntmpfs /mnt/a\\040b tmpfs rw 0 0\n";
        assert_eq!(tmpfs_paths(mounts), ["/dev/shm", "/mnt/a b"]);
    }
}
