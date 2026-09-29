//! `Polygloss`: the GPUI app. `--version` prints the version; `--gate …` opens
//! the M2 gate window (see `polygloss_app::gate_shell`). Anything else prints
//! the usage and exits 2 until T3.1 adds the app shell.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("Polygloss {}", polygloss_core::VERSION);
        return ExitCode::SUCCESS;
    }
    match args.split_first() {
        Some((first, rest)) if first == "--gate" => polygloss_app::gate_shell::run(rest),
        _ => {
            eprintln!("{}", polygloss_app::gate_shell::USAGE);
            ExitCode::from(2)
        }
    }
}
