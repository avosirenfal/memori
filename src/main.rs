//! memori: explains where your RAM is going.
//!
//! Reads the same kernel sources as `free`, `smem`, `slabtop` and `arc_summary`, and
//! combines them into one report that accounts for every byte of RAM. The program is
//! layered: `sources` parse kernel files, `snapshot` gathers them, `analysis` turns them
//! into typed facts, and `render` turns those into text.

mod analysis;
mod cli;
mod collections;
mod render;
mod sections;
mod snapshot;
mod sources;
mod units;

use std::io::{self, Write};

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    cli.color().write_global();

    let snapshot = snapshot::Snapshot::collect()?;
    let report = analysis::analyze(&snapshot, &cli.settings());
    let text = render::render(&report, &cli.view());
    match anstream::stdout().lock().write_all(text.as_bytes()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}
