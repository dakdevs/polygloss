//! `polygloss-cli` (on `PATH` as `polygloss`): human CLI, JSON CLI, `polygloss mcp`
//! and `polygloss wait` (design §14). Never links GPUI, lumis or tree-sitter.
//!
//! Exit codes: 0 success, 1 error (in JSON mode with `{"error": {"code",
//! "message"}}` on stdout), 2 a usage error from clap, or `polygloss wait`
//! waking the session. `mcp`, `wait` and `debug` keep stdout for their own
//! protocols and report errors on stderr only.
#![forbid(unsafe_code)]

mod cli;
mod commands;
mod debug;
mod debug_human;
mod output;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};
use commands::json::JsonCommand;
use output::{CliError, Mode, Report};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let global = cli.global;
    let mode = Mode::detect(global.json);
    let command = match cli.command {
        None => return report(mode, commands::live::run(cli.live, &global)),
        Some(command) => command,
    };
    let json = match command {
        Command::Show(args) => return report(mode, commands::show::run(args, &global)),
        Command::Compare(args) => return report(mode, commands::compare::run(args, &global)),
        Command::Open(args) => return report(mode, commands::open::run(args, &global)),
        Command::Snapshot(args) => return report(mode, commands::snapshot::run(args, &global)),
        Command::Mcp(args) => {
            return on_stderr(commands::mcp::run(args).map(|()| ExitCode::SUCCESS));
        }
        Command::Wait(args) => return on_stderr(commands::wait::run(args, &global)),
        Command::Debug(args) => {
            return on_stderr(debug::run(args, &global).map(|()| ExitCode::SUCCESS));
        }
        Command::Reviews(a) => JsonCommand::Reviews(a),
        Command::Threads(a) => JsonCommand::Threads(a),
        Command::Thread(a) => JsonCommand::Thread(a),
        Command::Reply(a) => JsonCommand::Reply(a),
        Command::Resolve(a) => JsonCommand::Resolve(a),
        Command::Unresolve(a) => JsonCommand::Unresolve(a),
        Command::Edit(a) => JsonCommand::Edit(a),
        Command::Delete(a) => JsonCommand::Delete(a),
        Command::Comment(a) => JsonCommand::Comment(a),
        Command::WaitReview(a) => JsonCommand::WaitReview(a),
        Command::Rereview(a) => JsonCommand::Rereview(a),
        Command::Focus(a) => JsonCommand::Focus(a),
    };
    // The JSON CLI always prints JSON.
    match commands::json::run(json, &global) {
        Ok(value) => {
            output::print_json(&value);
            ExitCode::SUCCESS
        }
        Err(err) => fail(Mode::Json, &err),
    }
}

/// Prints a human command's result or error.
fn report(mode: Mode, result: Result<Report, CliError>) -> ExitCode {
    match result {
        Ok(report) => {
            output::print_report(mode, &report);
            ExitCode::SUCCESS
        }
        Err(err) => fail(mode, &err),
    }
}

fn fail(mode: Mode, err: &CliError) -> ExitCode {
    output::print_error(mode, err);
    ExitCode::FAILURE
}

/// Commands whose stdout is not ours to write errors to.
fn on_stderr(result: anyhow::Result<ExitCode>) -> ExitCode {
    result.unwrap_or_else(|err| {
        eprintln!("polygloss: {err:#}");
        ExitCode::FAILURE
    })
}
