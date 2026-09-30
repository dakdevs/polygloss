//! `polygloss-cli` (on `PATH` as `polygloss`): human CLI, JSON CLI, `polygloss mcp`
//! and `polygloss wait`. Never links GPUI, lumis or tree-sitter.
#![forbid(unsafe_code)]

mod commands;
mod debug;
mod debug_human;

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
    /// Run the stdio MCP server for coding agents.
    Mcp(commands::mcp::McpArgs),
    /// Wait for the human to submit a review assigned to the session (Stop hook).
    /// Exit 0: nothing to report; 2: submitted (summary on stderr); 1: error.
    Wait(commands::wait::WaitArgs),
    /// Developer tools (git parity checks).
    #[command(hide = true)]
    Debug(debug::DebugArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Mcp(args)) => commands::mcp::run(args),
        Some(Command::Wait(args)) => return commands::wait::run(args),
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
