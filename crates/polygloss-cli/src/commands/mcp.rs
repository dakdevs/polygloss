//! `polygloss mcp [--channel]`: the stdio MCP server (design §15, T4.4).
//!
//! stdout carries JSON-RPC only; logs go to stderr without ANSI colors, filtered
//! by `RUST_LOG` (default `warn`, since rmcp logs at INFO when the stream ends).

use clap::Args;
use polygloss_mcp::ServeOptions;
use tracing_subscriber::EnvFilter;

/// `polygloss mcp` arguments.
#[derive(Debug, Clone, Args)]
pub struct McpArgs {
    /// Opt in to `claude/channel` push notifications (also `POLYGLOSS_MCP_CHANNEL=1`).
    #[arg(long)]
    pub channel: bool,
}

/// Runs the server on a current-thread tokio runtime until stdin closes.
pub fn run(args: McpArgs) -> anyhow::Result<()> {
    init_tracing();
    let opts = ServeOptions::from_flag_and_env(args.channel, |k| std::env::var(k).ok());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    runtime.block_on(polygloss_mcp::serve_stdio(opts))
}

/// stderr-only tracing, `RUST_LOG` or `warn`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
