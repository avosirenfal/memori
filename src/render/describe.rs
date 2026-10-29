//! Plain-language descriptions of common kernel slab caches, by name.

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

pub fn describe(name: &str) -> Option<&'static str> {
    DESCRIPTIONS
        .iter()
        .find(|(pattern, _)| pattern.matches(name))
        .map(|(_, description)| *description)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_known_caches() {
        assert_eq!(describe("kmalloc-cg-64"), Some("kernel allocations (per cgroup)"));
        assert_eq!(describe("squashfs_inode_cache"), Some("file metadata (inodes)"));
        assert_eq!(describe("some_driver_cache"), None);
    }
}
