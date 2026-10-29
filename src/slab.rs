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
    /// Whether the kernel counts this cache as reclaimable (SReclaimable); `None` if unknown.
    pub reclaimable: Option<bool>,
}

impl SlabCache {
    pub fn in_use(&self) -> Bytes {
        self.object_size.times(self.active_objects)
    }

    /// Slab memory that holds no live object: free slots in partly filled slabs, plus padding.
    pub fn wasted(&self) -> Bytes {
        self.footprint.saturating_sub(self.in_use())
    }

    pub fn description(&self) -> Option<&'static str> {
        describe(&self.name)
    }
}

/// Parses /proc/slabinfo; `reclaimable` says whether a cache (by name) is reclaimable.
pub fn parse(
    text: &str,
    page_size: Bytes,
    reclaimable: impl Fn(&str) -> Option<bool>,
) -> anyhow::Result<Vec<SlabCache>> {
    let mut lines = text.lines();
    let version = lines.next().context("empty file")?;
    ensure!(version.starts_with("slabinfo - version: 2."), "unknown format: {version}");
    lines
        .filter(|line| !line.starts_with('#'))
        .map(|line| parse_line(line, page_size, &reclaimable))
        .collect()
}

/// `name active_objs num_objs objsize objperslab pagesperslab : tunables .. : slabdata active_slabs num_slabs sharedavail`
fn parse_line(
    line: &str,
    page_size: Bytes,
    reclaimable: &impl Fn(&str) -> Option<bool>,
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
        reclaimable: reclaimable(fields[0]),
    })
}

/// SLUB publishes each cache's reclaimability in sysfs.
pub fn reclaimable_from_sysfs(name: &str) -> Option<bool> {
    fs::read_to_string(format!("/sys/kernel/slab/{name}/reclaim_account"))
        .ok()
        .map(|flag| flag.trim() == "1")
}

enum Pattern {
    Exact(&'static str),
    Prefix(&'static str),
    Suffix(&'static str),
}

impl Pattern {
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Exact(exact) => name == *exact,
            Self::Prefix(prefix) => name.starts_with(prefix),
            Self::Suffix(suffix) => name.ends_with(suffix),
        }
    }
}

/// Plain-language descriptions of common caches; the first matching pattern wins.
const DESCRIPTIONS: &[(Pattern, &str)] = {
    use Pattern::{Exact, Prefix, Suffix};
    &[
        (Exact("dentry"), "file & directory names (dentries)"),
        (Exact("inode_cache"), "generic file metadata (inodes)"),
        (Exact("buffer_head"), "block-device buffer metadata"),
        (Exact("radix_tree_node"), "page cache index"),
        (Exact("xa_node"), "page cache index"),
        (Exact("maple_node"), "process memory-map index"),
        (Exact("vm_area_struct"), "process memory-map regions"),
        (Prefix("anon_vma"), "anonymous memory bookkeeping"),
        (Exact("mm_struct"), "process address spaces"),
        (Exact("task_struct"), "processes and threads"),
        (Exact("pid"), "process IDs"),
        (Exact("cred_jar"), "process credentials"),
        (Exact("signal_cache"), "per-process signal state"),
        (Exact("sighand_cache"), "per-process signal handlers"),
        (Exact("files_cache"), "per-process open-file tables"),
        (Exact("filp"), "open files"),
        (Exact("names_cache"), "path lookups in progress"),
        (Exact("mnt_cache"), "mount points"),
        (Exact("kernfs_node_cache"), "sysfs & cgroup entries"),
        (Exact("proc_inode_cache"), "/proc entries"),
        (Exact("shmem_inode_cache"), "tmpfs & shared-memory files"),
        (Exact("sock_inode_cache"), "sockets"),
        (Exact("ovl_inode"), "overlayfs files (containers)"),
        (Exact("vmap_area"), "vmalloc bookkeeping"),
        (Prefix("eventpoll"), "epoll watchers"),
        (Prefix("inotify"), "inotify watches"),
        (Prefix("fsnotify"), "file-change notifications"),
        (Prefix("skbuff"), "network packet buffers"),
        (Prefix("TCP"), "TCP sockets"),
        (Prefix("UDP"), "UDP sockets"),
        (Prefix("nf_conntrack"), "firewall connection tracking"),
        (Prefix("bio"), "block I/O requests"),
        (Prefix("lsm_"), "security-module labels"),
        (Prefix("selinux"), "SELinux labels"),
        (Prefix("avc_"), "SELinux decision cache"),
        (Prefix("kvm_"), "KVM virtual machines"),
        (Prefix("kmalloc-rcl-"), "general kernel allocations"),
        (Prefix("kmalloc-cg-"), "kernel allocations (per cgroup)"),
        (Prefix("kmalloc-"), "general kernel allocations"),
        (Prefix("dma-kmalloc-"), "kernel allocations for DMA"),
        (Prefix("zio_data_buf_"), "ZFS cached file data"),
        (Prefix("zio_buf_"), "ZFS cached metadata & I/O buffers"),
        (Exact("dnode_t"), "ZFS dnodes (file metadata)"),
        (Exact("dmu_buf_impl_t"), "ZFS dbufs (cached block handles)"),
        (Prefix("arc_buf"), "ZFS ARC headers"),
        (Exact("zfs_znode_cache"), "ZFS files (znodes)"),
        (Exact("sa_cache"), "ZFS file attributes"),
        (Exact("abd_t"), "ZFS buffer descriptors"),
        (Prefix("btrfs_inode"), "btrfs files (inodes)"),
        (Prefix("btrfs_"), "btrfs metadata"),
        (Exact("ext4_inode_cache"), "ext4 files (inodes)"),
        (Prefix("ext4_"), "ext4 metadata"),
        (Prefix("jbd2_"), "ext4 journal"),
        (Exact("xfs_inode"), "XFS files (inodes)"),
        (Prefix("xfs_"), "XFS metadata"),
        (Exact("nfs_inode_cache"), "NFS files"),
        (Exact("fuse_inode"), "FUSE files"),
        (Suffix("inode_cache"), "file metadata (inodes)"),
        (Prefix("Acpi-"), "ACPI firmware tables"),
    ]
};

fn describe(name: &str) -> Option<&'static str> {
    DESCRIPTIONS
        .iter()
        .find(|(pattern, _)| pattern.matches(name))
        .map(|(_, description)| *description)
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
        let caches = parse(SAMPLE, Bytes(4096), |name| Some(name == "dentry")).unwrap();
        let dentry = &caches[0];
        assert_eq!(dentry.footprint, Bytes(100 * 4096));
        assert_eq!(dentry.in_use(), Bytes(1000 * 192));
        assert_eq!(dentry.wasted(), Bytes(100 * 4096 - 1000 * 192));
        assert_eq!(dentry.reclaimable, Some(true));
        assert_eq!(caches[1].footprint, Bytes(3 * 8 * 4096));
    }

    #[test]
    fn describes_known_caches() {
        assert_eq!(describe("kmalloc-cg-64"), Some("kernel allocations (per cgroup)"));
        assert_eq!(describe("squashfs_inode_cache"), Some("file metadata (inodes)"));
        assert_eq!(describe("some_driver_cache"), None);
    }
}
