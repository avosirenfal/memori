//! Per-process memory from /proc/<pid>/smaps_rollup, the same data `smem` reports.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use super::{byte_fields, parse_colon_kv};
use crate::collections::{NonEmpty, sorted_by};

byte_fields! {
    pub struct Rollup {
        /// Proportional set size: memory in RAM, with each shared page split evenly among
        /// the processes that map it, so these add up across processes.
        pss <- "Pss",
        /// This process's proportional share of its memory that has been swapped out.
        swap <- "SwapPss",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    /// When the process started, in clock ticks since boot.
    pub started: u64,
    /// The executable's file name, so that all of a program's processes group together.
    pub name: String,
    /// The command line; kernel-launched helpers may have none.
    pub command: Option<String>,
    pub mem: Rollup,
}

#[derive(Debug, Clone)]
pub struct ProcessScan {
    pub processes: Vec<Process>,
    /// Processes owned by other users, whose memory maps only root may read.
    pub unreadable: usize,
}

/// All of one program's processes.
#[derive(Debug, Clone)]
pub struct ProgramGroup<'a> {
    members: NonEmpty<&'a Process>,
}

impl<'a> ProgramGroup<'a> {
    pub fn name(&self) -> &'a str {
        &self.members.first().name
    }

    /// The oldest process whose parent isn't in the group: the one that started the rest.
    pub fn root(&self) -> &'a Process {
        let pids: HashSet<u32> = self.members.iter().map(|p| p.pid).collect();
        let roots = self.members.iter().filter(|p| !pids.contains(&p.parent));
        roots.min_by_key(|p| (p.started, p.pid)).copied().unwrap_or(self.members.first())
    }

    pub fn processes(&self) -> usize {
        self.members.len()
    }

    pub fn mem(&self) -> Rollup {
        self.members.iter().map(|p| p.mem).sum()
    }
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
    let read = |file: &str| fs::read_to_string(dir.join(file));
    match read("smaps_rollup") {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Probe::Unreadable,
        Err(_) => Probe::Skipped,
        Ok(rollup) if rollup.is_empty() => Probe::Skipped,
        Ok(rollup) => {
            let stat = read("stat").ok().and_then(|stat| parse_stat(&stat));
            let name = program_name(&dir);
            stat.zip(name).map_or(Probe::Skipped, |(Stat { parent, started }, name)| {
                Probe::Readable(Process {
                    pid,
                    parent,
                    started,
                    name,
                    command: fs::read(dir.join("cmdline")).ok().and_then(|raw| parse_cmdline(&raw)),
                    mem: Rollup::from_kv(&parse_colon_kv(&rollup)),
                })
            })
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Stat {
    parent: u32,
    started: u64,
}

/// /proc/<pid>/stat. The command name in parentheses may contain spaces, so fields are
/// counted from its closing parenthesis (field 3 onward).
fn parse_stat(stat: &str) -> Option<Stat> {
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Stat { parent: fields.get(1)?.parse().ok()?, started: fields.get(19)?.parse().ok()? })
}

/// /proc/<pid>/cmdline separates arguments with NUL bytes.
fn parse_cmdline(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw).replace('\0', " ").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The executable's file name, falling back to the kernel's 15-character `comm`.
fn program_name(dir: &Path) -> Option<String> {
    fs::read_link(dir.join("exe"))
        .ok()
        .and_then(|exe| Some(exe.file_name()?.to_string_lossy().into_owned()))
        .map(|name| unwrap_nix_name(name.trim_end_matches(" (deleted)")).to_owned())
        .or_else(|| Some(fs::read_to_string(dir.join("comm")).ok()?.trim().to_owned()))
}

/// Nix wraps programs in a script `foo` that runs the real binary, `.foo-wrapped`.
fn unwrap_nix_name(name: &str) -> &str {
    name.strip_prefix('.').and_then(|bare| bare.strip_suffix("-wrapped")).unwrap_or(name)
}

/// Groups processes by program name.
pub fn by_program(processes: &[Process]) -> Vec<ProgramGroup<'_>> {
    let by_name = sorted_by(processes.iter().collect(), |a, b| a.name.cmp(&b.name));
    by_name
        .chunk_by(|a, b| a.name == b.name)
        .filter_map(|members| NonEmpty::from_vec(members.to_vec()))
        .map(|members| ProgramGroup { members })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Bytes;

    fn process(pid: u32, parent: u32, name: &str, pss: u64) -> Process {
        Process {
            pid,
            parent,
            started: u64::from(pid),
            name: name.to_owned(),
            command: None,
            mem: Rollup { pss: Bytes(pss), ..Rollup::default() },
        }
    }

    #[test]
    fn groups_by_program_under_the_root_process() {
        let processes = [process(5, 1, "a", 10), process(2, 1, "b", 50), process(9, 5, "a", 60)];
        let groups = by_program(&processes);
        assert_eq!(groups[0].name(), "a");
        assert_eq!(groups[0].root().pid, 5);
        assert_eq!(groups[0].processes(), 2);
        assert_eq!(groups[0].mem().pss, Bytes(70));
        assert_eq!(groups[1].name(), "b");
    }

    #[test]
    fn reads_stat_with_spaces_in_the_name() {
        let stat = "42 (Web Content) S 7 42 42 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 1234 0";
        assert_eq!(parse_stat(stat), Some(Stat { parent: 7, started: 1234 }));
    }

    #[test]
    fn reads_cmdline() {
        assert_eq!(parse_cmdline(b"ls\0-l\0"), Some("ls -l".to_owned()));
        assert_eq!(parse_cmdline(b""), None);
    }

    #[test]
    fn unwraps_nix_wrapper_names() {
        assert_eq!(unwrap_nix_name(".kwrite-wrapped"), "kwrite");
        assert_eq!(unwrap_nix_name("firefox"), "firefox");
    }
}
