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
mod table;
mod tmpfs;
mod units;
mod zfs;

use std::io::{self, Write};

use clap::{ArgAction, Args, ColorChoice, Parser};

use report::SectionKind;

/// Explains where your RAM is going.
///
/// By default prints the overview and the programs table. Name sections to print only
/// those. Run with sudo to include other users' processes and kernel slab details.
#[derive(Parser)]
#[command(version, disable_help_flag = true)]
struct Cli {
    #[command(flatten)]
    sections: Sections,

    /// Print every section, and break the overview's totals down
    #[arg(short, long)]
    detailed: bool,

    /// List all programs instead of the largest ones
    #[arg(short, long)]
    all: bool,

    /// How many programs to list
    #[arg(long, value_name = "N", default_value_t = 15, conflicts_with = "all")]
    top: usize,

    /// How many kernel slab caches to list
    #[arg(long, value_name = "N", default_value_t = 12)]
    slabs: usize,

    /// List individual processes instead of grouping them by program
    #[arg(short = 'P', long)]
    per_process: bool,

    /// Print the title, section headings and column headers (the default)
    #[arg(short = 'h', long, overrides_with = "no_headers")]
    headers: bool,

    /// Leave out the title, section headings and column headers
    #[arg(short = 'H', long, overrides_with = "headers")]
    no_headers: bool,

    /// Leave out the explanations next to rows
    #[arg(short, long)]
    no_help: bool,

    /// When to use colors
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    /// Print help
    #[arg(long, action = ArgAction::Help)]
    help: Option<bool>,
}

/// Each flag selects one section.
#[derive(Args)]
struct Sections {
    /// Overview: where RAM is, and how much is available
    #[arg(short, long)]
    overview: bool,

    /// Programs and their memory
    #[arg(short, long)]
    programs: bool,

    /// ZFS ARC
    #[arg(short, long)]
    zfs: bool,

    /// Kernel memory and slab caches
    #[arg(short, long)]
    kernel: bool,

    /// File cache and shared memory
    #[arg(short, long)]
    cache: bool,

    /// Swap and zram
    #[arg(short, long)]
    swap: bool,

    /// Free memory fragmentation
    #[arg(short, long)]
    fragmentation: bool,

    /// Memory no kernel counter accounts for
    #[arg(short, long)]
    unexplained: bool,
}

impl Sections {
    fn chosen(&self) -> Vec<SectionKind> {
        SectionKind::ALL
            .into_iter()
            .filter(|kind| match kind {
                SectionKind::Overview => self.overview,
                SectionKind::Programs => self.programs,
                SectionKind::Zfs => self.zfs,
                SectionKind::Kernel => self.kernel,
                SectionKind::Cache => self.cache,
                SectionKind::Swap => self.swap,
                SectionKind::Fragmentation => self.fragmentation,
                SectionKind::Unexplained => self.unexplained,
            })
            .collect()
    }
}

impl Cli {
    fn options(&self) -> report::Options {
        let chosen = self.sections.chosen();
        let sections = match (chosen.is_empty(), self.detailed) {
            (false, _) => chosen,
            (true, true) => SectionKind::ALL.to_vec(),
            (true, false) => SectionKind::SUMMARY.to_vec(),
        };
        report::Options {
            sections,
            detailed: self.detailed,
            programs: (!self.all).then_some(self.top),
            slabs: self.slabs,
            per_process: self.per_process,
            show: table::Show { headers: !self.no_headers, help: !self.no_help },
        }
    }
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
    let text = report::render(&snapshot, &cli.options());
    match anstream::stdout().lock().write_all(text.as_bytes()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}
