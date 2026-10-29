//! Per-process memory from /proc/<pid>/smaps_rollup, the same data `smem` reports.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::procfs::{byte_fields, parse_colon_kv, sorted_by_key};
use crate::units::Bytes;

byte_fields! {
    pub struct Rollup {
        rss <- "Rss",
        pss <- "Pss",
        pss_anon <- "Pss_Anon",
        pss_file <- "Pss_File",
        pss_shmem <- "Pss_Shmem",
        private_clean <- "Private_Clean",
        private_dirty <- "Private_Dirty",
        /// This process's proportional share of swapped-out memory.
        swap <- "SwapPss",
    }
}

impl Rollup {
    /// USS: memory no other process maps — roughly what exiting would give back.
    pub fn unique(&self) -> Bytes {
        self.private_clean + self.private_dirty
    }
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
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
    pub members: Vec<&'a Process>,
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
        Ok(text) => Probe::Readable(Process {
            pid,
            name: program_name(&dir),
            cmdline: fs::read(dir.join("cmdline"))
                .map(|raw| String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_owned())
                .unwrap_or_default(),
            mem: Rollup::from_kv(&parse_colon_kv(&text)),
        }),
    }
}

/// The executable's file name, falling back to the kernel's 15-character `comm`.
fn program_name(dir: &Path) -> String {
    fs::read_link(dir.join("exe"))
        .ok()
        .and_then(|exe| Some(exe.file_name()?.to_string_lossy().into_owned()))
        .map(|name| name.trim_end_matches(" (deleted)").to_owned())
        .or_else(|| Some(fs::read_to_string(dir.join("comm")).ok()?.trim().to_owned()))
        .unwrap_or_else(|| "?".to_owned())
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
            mem: members.iter().map(|p| p.mem).sum(),
            members,
        })
        .collect();
    sorted_by_key(groups, |group| std::cmp::Reverse(group.mem.pss))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, pss: u64) -> Process {
        Process {
            pid,
            name: name.to_owned(),
            cmdline: String::new(),
            mem: Rollup { pss: Bytes(pss), private_dirty: Bytes(pss / 2), ..Rollup::default() },
        }
    }

    #[test]
    fn groups_and_sums_by_program() {
        let processes = [process(1, "a", 10), process(2, "b", 50), process(3, "a", 60)];
        let groups = by_program(&processes);
        assert_eq!(groups[0].name, "a");
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].mem.pss, Bytes(70));
        assert_eq!(groups[0].mem.unique(), Bytes(35));
        assert_eq!(groups[1].name, "b");
    }
}
