//! memori: explains where your RAM is going.
//!
//! Reads the same kernel sources as `free`, `smem`, `slabtop` and `arc_summary`, and
//! combines them into one report that accounts for every byte of RAM.

mod breakdown;
mod buddy;
mod gpu;
mod meminfo;
mod procfs;
mod procs;
mod report;
mod slab;
mod snapshot;
mod source;
mod swap;
mod tmpfs;
mod units;
mod zfs;

use std::io::{self, Write};

use clap::{ColorChoice, Parser};

/// Explains where your RAM is going, in plain language.
///
/// Run with sudo to include other users' processes and kernel slab details.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// How many programs to list
    #[arg(short = 'n', long, default_value_t = 15)]
    programs: usize,

    /// How many kernel slab caches to list (with --detailed)
    #[arg(short, long, default_value_t = 12)]
    slabs: usize,

    /// List individual processes instead of grouping them by program
    #[arg(short, long)]
    per_process: bool,

    /// Break every total down and add sections for ZFS, kernel slab, caches, swap and
    /// fragmentation
    #[arg(short, long)]
    detailed: bool,

    /// When to use colors
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.color {
        ColorChoice::Auto => anstream::ColorChoice::Auto,
        ColorChoice::Always => anstream::ColorChoice::Always,
        ColorChoice::Never => anstream::ColorChoice::Never,
    }
    .write_global();

    let snapshot = snapshot::Snapshot::collect()?;
    let options = report::Options {
        programs: cli.programs,
        slabs: cli.slabs,
        per_process: cli.per_process,
        detailed: cli.detailed,
    };
    let text = report::render(&snapshot, &options);
    match anstream::stdout().lock().write_all(text.as_bytes()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}
