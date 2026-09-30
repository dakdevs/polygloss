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
//! are accepted before or after the subcommand. Parse with
//! [`Cli::try_parse_args`], which also rejects the live review's `--since` and
//! `<PATH>` next to a subcommand.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Args, CommandFactory as _, Parser, Subcommand};

use crate::output::Mode;

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
/// the current directory) against its merge-base with the default branch. A
/// directory named like a subcommand needs a `./` prefix.
//
// Not `args_conflicts_with_subcommands`: with it, any root argument parsed
// first (a global flag included) stops clap from matching subcommands, so
// `polygloss --json show HEAD` would read `show` as the live review's PATH.
// Subcommand names win instead, and `try_parse_args` rejects `--since`/PATH
// next to a subcommand.
#[derive(Debug, Parser)]
#[command(name = "polygloss", version = polygloss_core::VERSION)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(flatten)]
    pub live: LiveArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Parses `args` (argv, program name first): clap, then the live review's
    /// arguments never combine with a subcommand.
    pub fn try_parse_args<I, T>(args: I) -> Result<Cli, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let cli = Cli::try_parse_from(args)?;
        if let Some(command) = &cli.command {
            let live = [
                cli.live.since.as_ref().map(|_| "--since".to_owned()),
                cli.live
                    .path
                    .as_ref()
                    .map(|p| format!("PATH '{}'", p.display())),
            ];
            let live: Vec<String> = live.into_iter().flatten().collect();
            if !live.is_empty() {
                return Err(Cli::command().error(
                    ErrorKind::ArgumentConflict,
                    format!(
                        "the live review's {} cannot be used with the subcommand '{}'",
                        live.join(" and "),
                        command.name()
                    ),
                ));
            }
        }
        Ok(cli)
    }
}

/// How a usage error (a [`clap::Error`] other than `--help`/`--version`) is
/// reported for `args`: `None` leaves it to clap (plain text on stderr), for
/// `mcp`, `wait` and `debug`, whose stdout is a protocol, and in human mode;
/// `Some(Mode::Json)` prints `{"error": {"code": "invalid_args", "message"}}`
/// on stdout, for the JSON CLI and, like results, when `--json` is given or
/// stdout is not a terminal.
pub fn usage_error_mode(args: &[OsString], stdout_is_tty: bool) -> Option<Mode> {
    let json_flag = args
        .iter()
        .skip(1)
        .take_while(|a| a.as_os_str() != "--")
        .any(|a| a.as_os_str() == "--json");
    match usage_subcommand(args).as_deref() {
        Some("mcp" | "wait" | "debug") => None,
        Some(name) if Command::is_json_cli(name) => Some(Mode::Json),
        _ => match Mode::choose(json_flag, stdout_is_tty) {
            Mode::Json => Some(Mode::Json),
            Mode::Human => None,
        },
    }
}

/// Best effort: the subcommand `args` meant, even though parsing failed.
pub fn usage_subcommand(args: &[OsString]) -> Option<String> {
    Cli::command()
        .ignore_errors(true)
        .try_get_matches_from(args)
        .ok()?
        .subcommand_name()
        .map(str::to_owned)
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

impl Command {
    /// The subcommand's name on the command line (`wait-review`).
    pub fn name(&self) -> &'static str {
        match self {
            Command::Show(_) => "show",
            Command::Compare(_) => "compare",
            Command::Open(_) => "open",
            Command::Snapshot(_) => "snapshot",
            Command::Mcp(_) => "mcp",
            Command::Wait(_) => "wait",
            Command::Reviews(_) => "reviews",
            Command::Threads(_) => "threads",
            Command::Thread(_) => "thread",
            Command::Reply(_) => "reply",
            Command::Resolve(_) => "resolve",
            Command::Unresolve(_) => "unresolve",
            Command::Edit(_) => "edit",
            Command::Delete(_) => "delete",
            Command::Comment(_) => "comment",
            Command::WaitReview(_) => "wait-review",
            Command::Rereview(_) => "rereview",
            Command::Focus(_) => "focus",
            Command::Debug(_) => "debug",
        }
    }

    /// The JSON CLI's commands, which always print JSON.
    pub const JSON_CLI: [&'static str; 12] = [
        "reviews",
        "threads",
        "thread",
        "reply",
        "resolve",
        "unresolve",
        "edit",
        "delete",
        "comment",
        "wait-review",
        "rereview",
        "focus",
    ];

    pub fn is_json_cli(name: &str) -> bool {
        Command::JSON_CLI.contains(&name)
    }
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

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_args(std::iter::once("polygloss").chain(args.iter().copied()))
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        std::iter::once("polygloss")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn command_tree_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_parse_after_subcommands() {
        let cli = parse(&["show", "HEAD", "--json", "--no-open", "--repo", "/r"]).expect("parse");
        assert!(cli.global.json && cli.global.no_open);
        assert_eq!(cli.global.repo, Some(PathBuf::from("/r")));
        assert!(matches!(cli.command, Some(Command::Show(ShowArgs { ref rev })) if rev == "HEAD"));
    }

    #[test]
    fn global_flags_parse_before_subcommands() {
        let cli = parse(&["--json", "--no-open", "show", "HEAD"]).expect("show");
        assert!(cli.global.json && cli.global.no_open);
        assert!(matches!(cli.command, Some(Command::Show(ShowArgs { ref rev })) if rev == "HEAD"));
        assert!(cli.live.path.is_none());

        let cli = parse(&["--repo", "/x", "snapshot"]).expect("snapshot");
        assert_eq!(cli.global.repo, Some(PathBuf::from("/x")));
        assert!(matches!(
            cli.command,
            Some(Command::Snapshot(SnapshotArgs { path: None, .. }))
        ));

        let cli = parse(&["--session", "s1", "wait"]).expect("wait");
        assert_eq!(cli.global.session.as_deref(), Some("s1"));
        assert!(matches!(cli.command, Some(Command::Wait(_))));

        let cli = parse(&["--no-open", "open", "deadbeef"]).expect("open");
        assert!(cli.global.no_open);
        assert!(
            matches!(cli.command, Some(Command::Open(OpenArgs { ref diff })) if diff == "deadbeef")
        );

        let cli = parse(&["--agent", "x", "reviews"]).expect("reviews");
        assert_eq!(cli.global.agent.as_deref(), Some("x"));
        assert!(matches!(cli.command, Some(Command::Reviews(_))));

        let cli = parse(&["--json", "mcp", "--channel"]).expect("mcp");
        assert!(matches!(
            cli.command,
            Some(Command::Mcp(McpArgs { channel: true }))
        ));

        // Mixed: some before, some after.
        let cli =
            parse(&["--json", "compare", "a", "b", "--no-open", "--repo", "/r"]).expect("compare");
        assert!(cli.global.json && cli.global.no_open);
        assert!(matches!(cli.command, Some(Command::Compare(_))));
    }

    #[test]
    fn subcommand_help_after_a_global_flag_is_the_subcommand_help() {
        let err = parse(&["--json", "mcp", "--help"]).expect_err("help");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        assert!(err.to_string().contains("--channel"), "{err}");
    }

    #[test]
    fn bare_invocation_is_the_live_review() {
        let cli = parse(&["--since", "HEAD", "some/dir"]).expect("parse");
        assert!(cli.command.is_none());
        assert_eq!(cli.live.since.as_deref(), Some("HEAD"));
        assert_eq!(cli.live.path, Some(PathBuf::from("some/dir")));

        let cli = parse(&["--json", "--no-open", "some/dir", "--repo", "/r"]).expect("flags");
        assert!(cli.command.is_none());
        assert_eq!(cli.live.path, Some(PathBuf::from("some/dir")));
        assert!(cli.global.json && cli.global.no_open);

        // A directory named like a subcommand needs `./`.
        let cli = parse(&["./show"]).expect("dir");
        assert!(cli.command.is_none());
        assert_eq!(cli.live.path, Some(PathBuf::from("./show")));
    }

    #[test]
    fn live_arguments_never_combine_with_a_subcommand() {
        for args in [
            &["--since", "HEAD", "show", "x"][..],
            &["--json", "--since", "HEAD", "wait"],
            &["some/dir", "show", "x"],
            &["--no-open", "some/dir", "snapshot"],
        ] {
            let err = parse(args).expect_err(&format!("{args:?}"));
            assert_eq!(err.kind(), ErrorKind::ArgumentConflict, "{args:?}: {err}");
            assert!(err.to_string().contains("live review"), "{err}");
        }
        // The subcommand's own --since is fine.
        let cli = parse(&["snapshot", "--since", "HEAD"]).expect("snapshot --since");
        assert!(
            matches!(cli.command, Some(Command::Snapshot(SnapshotArgs { ref since, .. })) if since.as_deref() == Some("HEAD"))
        );
    }

    #[test]
    fn wait_takes_the_global_session() {
        let cli = parse(&["wait", "--session", "abc", "--timeout", "90"]).expect("parse");
        assert_eq!(cli.global.session.as_deref(), Some("abc"));
        let Some(Command::Wait(args)) = cli.command else {
            panic!("not wait");
        };
        assert_eq!(args.timeout, Some(90));
    }

    #[test]
    fn command_names_match_the_tree() {
        let tree: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        for name in Command::JSON_CLI {
            assert!(tree.iter().any(|t| t == name), "{name}");
        }
        let cli = parse(&["wait-review", "r1"]).expect("wait-review");
        assert_eq!(cli.command.expect("command").name(), "wait-review");
        let cli = parse(&["snapshot"]).expect("snapshot");
        assert_eq!(cli.command.expect("command").name(), "snapshot");
    }

    #[test]
    fn usage_errors_are_json_for_the_json_cli_and_json_mode() {
        // The JSON CLI always prints JSON, flags before or after.
        assert_eq!(usage_error_mode(&os(&["threads"]), true), Some(Mode::Json));
        assert_eq!(
            usage_error_mode(&os(&["--agent", "a", "reply", "t1", "--bogus"]), true),
            Some(Mode::Json)
        );
        // Human commands follow --json and the terminal.
        assert_eq!(usage_error_mode(&os(&["show"]), true), None);
        assert_eq!(usage_error_mode(&os(&["show"]), false), Some(Mode::Json));
        assert_eq!(
            usage_error_mode(&os(&["--json", "show"]), true),
            Some(Mode::Json)
        );
        assert_eq!(
            usage_error_mode(&os(&["show", "--json"]), true),
            Some(Mode::Json)
        );
        assert_eq!(usage_error_mode(&os(&["--bogus"]), false), Some(Mode::Json));
        assert_eq!(usage_error_mode(&os(&["show", "--", "--json"]), true), None);
        assert_eq!(
            usage_subcommand(&os(&["--repo", "/r", "wait", "--bogus"])).as_deref(),
            Some("wait")
        );
        assert_eq!(usage_subcommand(&os(&["--bogus"])), None);
        // stdout is the protocol of mcp, wait and debug: clap's own text.
        for sub in ["mcp", "wait", "debug"] {
            assert_eq!(
                usage_error_mode(&os(&["--json", sub, "--bogus"]), false),
                None,
                "{sub}"
            );
        }
    }
}
