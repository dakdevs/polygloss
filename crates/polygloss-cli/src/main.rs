//! `polygloss-cli` (on `PATH` as `polygloss`): human CLI, JSON CLI, `polygloss mcp`
//! and `polygloss wait`. Never links GPUI, lumis or tree-sitter.
#![forbid(unsafe_code)]

use clap::Parser;

/// Local diff reviewer for humans and coding agents.
#[derive(Debug, Parser)]
#[command(name = "polygloss", version = polygloss_core::VERSION)]
struct Cli {}

fn main() {
    let Cli {} = Cli::parse();
}
