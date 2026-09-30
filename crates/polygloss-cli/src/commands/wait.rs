//! `polygloss wait [--session <id>] [--timeout <s>]`: the `asyncRewake` hook
//! waiter (design §16.3). T4.3 wires the command; T4.8 implements it.
//!
//! Exit codes: 0 nothing to report (no assigned review, or the deadline is
//! near), 2 a review was submitted (summary on stderr, wakes the session), 1
//! an error. The session comes from the global `--session`, else the hook's
//! stdin JSON `session_id`, else the environment.

use std::process::ExitCode;

use clap::Args;

use crate::cli::GlobalArgs;

/// `polygloss wait` arguments (`--session` is the global flag).
#[derive(Debug, Clone, Args)]
pub struct WaitArgs {
    /// The hook's timeout in seconds (default 3600, or
    /// $POLYGLOSS_WAIT_TIMEOUT_S); the waiter exits 0 about 30 s before it.
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u64>,
}

/// Runs the waiter. Errors exit 1 with the message on stderr.
pub fn run(_args: WaitArgs, _global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`polygloss wait` is not implemented in this build")
}
