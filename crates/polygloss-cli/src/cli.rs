//! The complete `polygloss` command tree (design §14, T4.3). Handlers live in
//! `commands::*`; T4.8 fills `wait` and T4.9 the JSON commands.
//!
//! ```text
//! polygloss [--since merge-base|HEAD|<rev>] [<path>]      live review of a worktree
//! polygloss show <rev> | compare <base> <head> [--direct] [--label <text>]
//! polygloss open <diff_id|prefix> | snapshot [<path>]
//! polygloss mcp [--channel] | wait [--session <id>] [--timeout <s>]
//! polygloss reviews | threads | thread | reply | resolve | unresolve | edit
//!           | delete | comment | wait-review | rereview | focus   (JSON CLI)
//! ```
//!
//! The global flags (`--repo`, `--json`, `--no-open`, `--agent`, `--session`)
//! are accepted before or after the subcommand.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::commands::json::{
    CommentArgs, DeleteArgs, EditArgs, FocusArgs, ReplyArgs, RereviewArgs, ResolveArgs,
    ReviewsArgs, ThreadArgs, ThreadsArgs, UnresolveArgs, WaitReviewArgs,
};
use crate::commands::mcp::McpArgs;
use crate::commands::wait::WaitArgs;
use crate::debug::DebugArgs;

/// Local diff reviewer for humans and coding agents.
///
/// Without a subcommand, opens a live review of the worktree at <PATH> (default:
/// the current directory) against its merge-base with the default branch.
#[derive(Debug, Parser)]
#[command(
    name = "polygloss",
    version = polygloss_core::VERSION,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(flatten)]
    pub live: LiveArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Flags every command accepts (design §14 "Global flags").
#[derive(Debug, Clone, Default, Args)]
pub struct GlobalArgs {
    /// The repository or worktree to use (any path inside it). Default: the
    /// current directory.
    #[arg(long, global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,
    /// Print JSON (the default when stdout is not a terminal).
    #[arg(long, global = true)]
    pub json: bool,
    /// Resolve and print the ids without opening or launching the app.
    #[arg(long, global = true)]
    pub no_open: bool,
    /// Author name for agent writes (default: $POLYGLOSS_AGENT, else `agent`).
    #[arg(long, global = true, value_name = "NAME")]
    pub agent: Option<String>,
    /// The agent session id (the hook waiter and the JSON CLI).
    #[arg(long, global = true, value_name = "ID")]
    pub session: Option<String>,
}

/// `polygloss [--since …] [<path>]`: the live review (no subcommand).
#[derive(Debug, Clone, Default, Args)]
pub struct LiveArgs {
    /// The base: `merge-base` (default: with the default branch), `HEAD`, or any
    /// revision.
    #[arg(long, value_name = "merge-base|HEAD|REV")]
    pub since: Option<String>,
    /// Any path inside the worktree to review (default: --repo, else the
    /// current directory).
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Review a commit against its first parent.
    Show(ShowArgs),
    /// Compare two branches or revisions (three-dot by default, like a pull
    /// request).
    Compare(CompareArgs),
    /// Open a diff by id or unique prefix (at least 8 hex digits).
    Open(OpenArgs),
    /// Pin the live state of a worktree as a new iteration of its live review.
    Snapshot(SnapshotArgs),
    /// Run the stdio MCP server for coding agents.
    Mcp(McpArgs),
    /// Hook waiter: exit 2 with a summary when an assigned review is submitted.
    Wait(WaitArgs),
    /// JSON CLI: list reviews (MCP `list_reviews`).
    Reviews(ReviewsArgs),
    /// JSON CLI: list a review's threads (MCP `list_threads`).
    Threads(ThreadsArgs),
    /// JSON CLI: one thread with its comments (MCP `get_thread`).
    Thread(ThreadArgs),
    /// JSON CLI: reply to a thread (MCP `reply`).
    Reply(ReplyArgs),
    /// JSON CLI: resolve a thread (MCP `resolve`).
    Resolve(ResolveArgs),
    /// JSON CLI: reopen a thread (MCP `unresolve`).
    Unresolve(UnresolveArgs),
    /// JSON CLI: edit your comment (MCP `edit_comment`).
    Edit(EditArgs),
    /// JSON CLI: delete your comment (MCP `delete_comment`).
    Delete(DeleteArgs),
    /// JSON CLI: create a note or question (MCP `create_comment`).
    Comment(CommentArgs),
    /// JSON CLI: wait for the human to submit a review (MCP `wait_for_review`).
    WaitReview(WaitReviewArgs),
    /// JSON CLI: ask the human to review again (MCP `request_rereview`).
    Rereview(RereviewArgs),
    /// JSON CLI: scroll the app to a location (MCP `focus`).
    Focus(FocusArgs),
    /// Developer and test tools.
    #[command(hide = true)]
    Debug(DebugArgs),
}

/// `polygloss show <rev>`.
#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    /// The commit (anything `git rev-parse` accepts).
    #[arg(value_name = "REV")]
    pub rev: String,
}

/// `polygloss compare <base> <head> [--direct] [--label <text>]`.
#[derive(Debug, Clone, Args)]
pub struct CompareArgs {
    #[arg(value_name = "BASE")]
    pub base: String,
    #[arg(value_name = "HEAD")]
    pub head: String,
    /// Compare the two trees as they are (two-dot) instead of the changes on
    /// HEAD since the merge-base.
    #[arg(long)]
    pub direct: bool,
    /// Display label such as `PR #123` (not part of the review's identity).
    #[arg(long, value_name = "TEXT")]
    pub label: Option<String>,
}

/// `polygloss open <diff_id|prefix>`.
#[derive(Debug, Clone, Args)]
pub struct OpenArgs {
    /// A diff id or a unique prefix of at least 8 hex digits.
    #[arg(value_name = "DIFF_ID")]
    pub diff: String,
}

/// `polygloss snapshot [<path>]`.
#[derive(Debug, Clone, Args)]
pub struct SnapshotArgs {
    /// The live review's base: `merge-base` (default), `HEAD` or a revision.
    #[arg(long, value_name = "merge-base|HEAD|REV")]
    pub since: Option<String>,
    /// Any path inside the worktree (default: --repo, else the current
    /// directory).
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn command_tree_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_parse_after_subcommands() {
        let cli = Cli::try_parse_from([
            "polygloss",
            "show",
            "HEAD",
            "--json",
            "--no-open",
            "--repo",
            "/r",
        ])
        .expect("parse");
        assert!(cli.global.json && cli.global.no_open);
        assert_eq!(cli.global.repo, Some(PathBuf::from("/r")));
        assert!(matches!(cli.command, Some(Command::Show(ShowArgs { ref rev })) if rev == "HEAD"));
    }

    #[test]
    fn bare_invocation_is_the_live_review() {
        let cli = Cli::try_parse_from(["polygloss", "--since", "HEAD", "some/dir"]).expect("parse");
        assert!(cli.command.is_none());
        assert_eq!(cli.live.since.as_deref(), Some("HEAD"));
        assert_eq!(cli.live.path, Some(PathBuf::from("some/dir")));
        // A live flag never combines with a subcommand.
        assert!(Cli::try_parse_from(["polygloss", "--since", "HEAD", "show", "x"]).is_err());
    }

    #[test]
    fn wait_takes_the_global_session() {
        let cli = Cli::try_parse_from(["polygloss", "wait", "--session", "abc", "--timeout", "90"])
            .expect("parse");
        assert_eq!(cli.global.session.as_deref(), Some("abc"));
        let Some(Command::Wait(args)) = cli.command else {
            panic!("not wait");
        };
        assert_eq!(args.timeout, Some(90));
    }
}
