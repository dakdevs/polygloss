//! `polygloss-cli` (on `PATH` as `polygloss`): human CLI, JSON CLI, `polygloss mcp`
//! and `polygloss wait` (design §14). Never links GPUI, lumis or tree-sitter.
//!
//! Exit codes: 0 success, 1 error (in JSON mode with `{"error": {"code",
//! "message"}}` on stdout), 2 a usage error from clap (in JSON mode with code
//! `invalid_args`; `wait` exits 1 instead), or `polygloss wait` waking the
//! session. `mcp`, `wait` and `debug` keep stdout for their own protocols and
//! report errors on stderr only.
#![forbid(unsafe_code)]

mod cli;
mod commands;
mod debug;
mod debug_human;
mod output;

use std::ffi::OsString;
use std::io::IsTerminal as _;
use std::process::ExitCode;

use cli::{Cli, Command};
use commands::json::JsonCommand;
use output::{CliError, Mode, Report};

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let cli = match Cli::try_parse_args(&args) {
        Ok(cli) => cli,
        Err(err) => return usage_error(err, &args),
    };
    let global = cli.global;
    let mode = Mode::detect(global.json);
    let command = match cli.command {
        None => return report(mode, commands::live::run(cli.live, &global)),
        Some(command) => command,
    };
    let other = match JsonCommand::from_command(command) {
        Ok(json) => return run_json(json, &global),
        Err(other) => *other,
    };
    match other {
        Command::Show(args) => report(mode, commands::show::run(args, &global)),
        Command::Compare(args) => report(mode, commands::compare::run(args, &global)),
        Command::Open(args) => report(mode, commands::open::run(args, &global)),
        Command::Snapshot(args) => report(mode, commands::snapshot::run(args, &global)),
        Command::Mcp(args) => on_stderr(commands::mcp::run(args).map(|()| ExitCode::SUCCESS)),
        Command::Wait(args) => on_stderr(commands::wait::run(args, &global)),
        Command::Debug(args) => on_stderr(debug::run(args, &global).map(|()| ExitCode::SUCCESS)),
        other => unreachable!("{other:?} is a JSON command"),
    }
}

/// Runs a JSON CLI command, which always prints JSON.
fn run_json(json: JsonCommand, global: &cli::GlobalArgs) -> ExitCode {
    match commands::json::run(json, global) {
        Ok(value) => {
            output::print_json(&value);
            ExitCode::SUCCESS
        }
        Err(err) => fail(Mode::Json, &err),
    }
}

/// `--help`/`--version` (exit 0) and usage errors (exit 2): clap's text, or in
/// JSON mode an `invalid_args` error object on stdout. `wait` exits 1 instead,
/// since its exit 2 wakes the agent's session (design §14, §16).
fn usage_error(err: clap::Error, args: &[OsString]) -> ExitCode {
    if !err.use_stderr() {
        err.exit();
    }
    if cli::usage_subcommand(args).as_deref() == Some("wait") {
        let _ = err.print();
        return ExitCode::FAILURE;
    }
    match cli::usage_error_mode(args, std::io::stdout().is_terminal()) {
        Some(Mode::Json) => {
            output::print_error(Mode::Json, &CliError::from_usage(&err));
            ExitCode::from(2)
        }
        _ => err.exit(),
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
