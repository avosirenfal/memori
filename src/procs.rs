//! Per-process memory from /proc/<pid>/smaps_rollup, the same data `smem` reports.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::procfs::{byte_fields, parse_colon_kv, sorted_by_key};

byte_fields! {
    pub struct Rollup {
        /// Proportional set size: memory in RAM, with each shared page split evenly among
        /// the processes that map it, so these add up across processes.
        pss <- "Pss",
        /// This process's proportional share of its memory that has been swapped out.
        swap <- "SwapPss",
    }
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// When the process started, in clock ticks since boot.
    pub started: u64,
    /// The executable's file name, so that all of a program's processes group together.
    pub name: String,
    pub cmdline: String,
    pub mem: Rollup,
}

#[derive(Debug, Clone)]
pub struct ProcessScan {
    pub processes: Vec<Process>,
    /// Processes owned by other users, whose memory maps only root may read.
    pub unreadable: usize,
}

/// A program's processes, summed.
#[derive(Debug, Clone)]
pub struct ProgramGroup<'a> {
    pub name: &'a str,
    /// The oldest process whose parent isn't part of the group: the one that started the rest.
    pub root: &'a Process,
    pub processes: usize,
    pub mem: Rollup,
}

enum Probe {
    Readable(Process),
    Unreadable,
    /// A kernel thread (no user memory), or a process that exited while we looked.
    Skipped,
}

pub fn scan() -> ProcessScan {
    let probes: Vec<Probe> = fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .map(probe)
        .collect();
    let unreadable = probes.iter().filter(|p| matches!(p, Probe::Unreadable)).count();
    let processes = probes
        .into_iter()
        .filter_map(|probe| match probe {
            Probe::Readable(process) => Some(process),
            Probe::Unreadable | Probe::Skipped => None,
        })
        .collect();
    ProcessScan { processes, unreadable }
}

fn probe(pid: u32) -> Probe {
    let dir = Path::new("/proc").join(pid.to_string());
    match fs::read_to_string(dir.join("smaps_rollup")) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Probe::Unreadable,
        Err(_) => Probe::Skipped,
        Ok(text) if text.is_empty() => Probe::Skipped,
        Ok(text) => {
            let (ppid, started) = fs::read_to_string(dir.join("stat"))
                .ok()
                .and_then(|stat| parse_stat(&stat))
                .unwrap_or((0, 0));
            Probe::Readable(Process {
                pid,
                ppid,
                started,
                name: program_name(&dir),
                cmdline: fs::read(dir.join("cmdline"))
                    .map(|raw| String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_owned())
                    .unwrap_or_default(),
                mem: Rollup::from_kv(&parse_colon_kv(&text)),
            })
        }
    }
}

/// Parent pid and start time from /proc/<pid>/stat. The command name in parentheses may
/// contain spaces, so fields are counted from its closing parenthesis (field 3 onward).
fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// The executable's file name, falling back to the kernel's 15-character `comm`.
fn program_name(dir: &Path) -> String {
    fs::read_link(dir.join("exe"))
        .ok()
        .and_then(|exe| Some(exe.file_name()?.to_string_lossy().into_owned()))
        .map(|name| unwrap_nix_name(name.trim_end_matches(" (deleted)")).to_owned())
        .or_else(|| Some(fs::read_to_string(dir.join("comm")).ok()?.trim().to_owned()))
        .unwrap_or_else(|| "?".to_owned())
}

/// Nix wraps programs in a script `foo` that runs the real binary, `.foo-wrapped`.
fn unwrap_nix_name(name: &str) -> &str {
    name.strip_prefix('.').and_then(|bare| bare.strip_suffix("-wrapped")).unwrap_or(name)
}

/// Groups processes by program name, largest proportional share first.
pub fn by_program(processes: &[Process]) -> Vec<ProgramGroup<'_>> {
    let grouped =
        processes.iter().fold(BTreeMap::<&str, Vec<&Process>>::new(), |mut groups, process| {
            groups.entry(&process.name).or_default().push(process);
            groups
        });
    let groups: Vec<ProgramGroup> = grouped
        .into_iter()
        .map(|(name, members)| ProgramGroup {
            name,
            root: root_of(&members),
            processes: members.len(),
            mem: members.iter().map(|p| p.mem).sum(),
        })
        .collect();
    sorted_by_key(groups, |group| std::cmp::Reverse(group.mem.pss))
}

/// The oldest member whose parent is outside the group. `members` is never empty.
fn root_of<'a>(members: &[&'a Process]) -> &'a Process {
    let pids: HashSet<u32> = members.iter().map(|p| p.pid).collect();
    members
        .iter()
        .copied()
        .filter(|p| !pids.contains(&p.ppid))
        .min_by_key(|p| (p.started, p.pid))
        .unwrap_or(members[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Bytes;

    fn process(pid: u32, ppid: u32, name: &str, pss: u64) -> Process {
        Process {
            pid,
            ppid,
            started: u64::from(pid),
            name: name.to_owned(),
            cmdline: String::new(),
            mem: Rollup { pss: Bytes(pss), ..Rollup::default() },
        }
    }

    #[test]
    fn groups_by_program_under_the_root_process() {
        let processes = [process(5, 1, "a", 10), process(2, 1, "b", 50), process(9, 5, "a", 60)];
        let groups = by_program(&processes);
        assert_eq!(groups[0].name, "a");
        assert_eq!(groups[0].root.pid, 5);
        assert_eq!(groups[0].processes, 2);
        assert_eq!(groups[0].mem.pss, Bytes(70));
        assert_eq!(groups[1].name, "b");
    }

    #[test]
    fn reads_stat_with_spaces_in_the_name() {
        let stat = "42 (Web Content) S 7 42 42 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 1234 0";
        assert_eq!(parse_stat(stat), Some((7, 1234)));
    }

    #[test]
    fn unwraps_nix_wrapper_names() {
        assert_eq!(unwrap_nix_name(".kwrite-wrapped"), "kwrite");
        assert_eq!(unwrap_nix_name("firefox"), "firefox");
    }
}
