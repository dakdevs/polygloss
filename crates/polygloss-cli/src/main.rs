//! `polygloss-cli` (on `PATH` as `polygloss`): human CLI, JSON CLI, `polygloss mcp`
//! and `polygloss wait`. Never links GPUI, lumis or tree-sitter.
#![forbid(unsafe_code)]

mod debug;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Local diff reviewer for humans and coding agents.
#[derive(Debug, Parser)]
#[command(name = "polygloss", version = polygloss_core::VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Developer tools (git parity checks).
    #[command(hide = true)]
    Debug(debug::DebugArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Debug(args)) => debug::run(args),
        None => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("polygloss: {err:#}");
            ExitCode::FAILURE
        }
    }
}
