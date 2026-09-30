//! The JSON CLI for agents without MCP (design §14 "JSON CLI", T4.9): one
//! command per MCP tool, each calling the same `polygloss_mcp::api` function
//! as its MCP twin and printing the same JSON (the tool's `structuredContent`).
//!
//! - **Bodies** come from `--body-file` / `--summary-file` (`-` = stdin) as
//!   UTF-8; trailing newlines are dropped (`echo … |` adds one).
//! - **Author name**: `--agent`, else `$POLYGLOSS_AGENT`, else `agent` (empty
//!   values fall through). It is the `author_name` of every write, so `edit`
//!   and `delete` work on comments written under the same name (OQ-30).
//! - **Session**: `--session`, else `$CLAUDE_CODE_SESSION_ID`, else a fresh
//!   `pg-<uuidv7>` per call. Writes upsert it first: a new row is named after
//!   the author and has no owner pid, so a CLI call never links the agent's
//!   sessions (§16.4); an existing row (e.g. the MCP server's) keeps its client
//!   and owner. `reviews --assigned me` needs an explicit session.
//! - **`--no-open`** never launches or talks to the app: `focus` checks its ids
//!   and reports `unavailable`, and `rereview` does not launch a closed app to
//!   notify (a running app still gets the `store_changed` nudge, as after every
//!   write).
//! - `focus <target>` is a diff id (or prefix) when `target` is all hex digits,
//!   else a review id (review ids are UUIDs, which have dashes).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, ValueEnum};
use polygloss_core::review::SessionInfo;
use polygloss_mcp::ApiContext;
use polygloss_mcp::api::shapes::{
    AgentThreadKind, AnchorParam, AssignedFilter, AuthorFilter, SideParam, ThreadKindParam,
    ThreadStatusFilter,
};
use polygloss_mcp::api::{self, WaitControl};
use polygloss_mcp::app_link::FocusStatus;
use polygloss_platform::launch::{Launcher, SystemLauncher};
use serde::Serialize;
use serde_json::Value;

use crate::cli::{Command, GlobalArgs};
use crate::output::CliError;

/// The author-name variable (design §14).
pub const AGENT_ENV: &str = "POLYGLOSS_AGENT";
/// The author name when neither `--agent` nor [`AGENT_ENV`] is set.
pub const DEFAULT_AGENT: &str = "agent";
/// The session variable Claude Code sets for its Bash tool (design §15.1).
pub const SESSION_ENV: &str = polygloss_mcp::session::SESSION_ENV;

/// One JSON CLI command with its arguments.
#[derive(Debug, Clone)]
pub enum JsonCommand {
    Reviews(ReviewsArgs),
    Threads(ThreadsArgs),
    Thread(ThreadArgs),
    Reply(ReplyArgs),
    Resolve(ResolveArgs),
    Unresolve(UnresolveArgs),
    Edit(EditArgs),
    Delete(DeleteArgs),
    Comment(CommentArgs),
    WaitReview(WaitReviewArgs),
    Rereview(RereviewArgs),
    Focus(FocusArgs),
}

/// `polygloss reviews` (`list_reviews`). `--repo` limits it to one repository.
#[derive(Debug, Clone, Args)]
pub struct ReviewsArgs {
    /// `open`, `changes_requested`, `commented`, `approved` or
    /// `rereview_requested`.
    #[arg(long)]
    pub status: Option<String>,
    /// `me`: only reviews assigned to --session.
    #[arg(long, value_enum)]
    pub assigned: Option<AssignedArg>,
    /// `next_cursor` of the previous page.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Page size (default 50).
    #[arg(long)]
    pub limit: Option<u32>,
}

/// `polygloss threads <review_id>` (`list_threads`).
#[derive(Debug, Clone, Args)]
pub struct ThreadsArgs {
    #[arg(value_name = "REVIEW_ID")]
    pub review_id: String,
    /// `open` (default), `resolved` or `all`.
    #[arg(long, value_enum)]
    pub status: Option<ThreadStatusArg>,
    /// Who started the thread.
    #[arg(long, value_enum)]
    pub author: Option<AuthorArg>,
    #[arg(long, value_enum)]
    pub kind: Option<ThreadKindArg>,
    /// Only threads on this file.
    #[arg(long)]
    pub path: Option<String>,
    /// Only threads with activity after this event seq.
    #[arg(long, value_name = "SEQ")]
    pub since: Option<i64>,
    /// `next_cursor` of the previous page.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Page size (default 50).
    #[arg(long)]
    pub limit: Option<u32>,
}

/// `polygloss thread <thread_id>` (`get_thread`).
#[derive(Debug, Clone, Args)]
pub struct ThreadArgs {
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: String,
}

/// `polygloss reply <thread_id> --body-file -` (`reply`).
#[derive(Debug, Clone, Args)]
pub struct ReplyArgs {
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: String,
    /// Markdown body; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    pub body_file: PathBuf,
    /// Also resolve the thread.
    #[arg(long)]
    pub resolve: bool,
}

/// `polygloss resolve <thread_id> [--body-file -]` (`resolve`).
#[derive(Debug, Clone, Args)]
pub struct ResolveArgs {
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: String,
    /// Optional closing reply; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    pub body_file: Option<PathBuf>,
}

/// `polygloss unresolve <thread_id>` (`unresolve`).
#[derive(Debug, Clone, Args)]
pub struct UnresolveArgs {
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: String,
}

/// `polygloss edit <comment_id> --body-file -` (`edit_comment`).
#[derive(Debug, Clone, Args)]
pub struct EditArgs {
    #[arg(value_name = "COMMENT_ID")]
    pub comment_id: String,
    /// The new markdown body; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    pub body_file: PathBuf,
}

/// `polygloss delete <comment_id>` (`delete_comment`).
#[derive(Debug, Clone, Args)]
pub struct DeleteArgs {
    #[arg(value_name = "COMMENT_ID")]
    pub comment_id: String,
}

/// `polygloss comment <review_id> --kind note|question [--path … --side …
/// --line … --start-line …] --body-file -` (`create_comment`).
#[derive(Debug, Clone, Args)]
pub struct CommentArgs {
    #[arg(value_name = "REVIEW_ID")]
    pub review_id: String,
    #[arg(long, value_enum)]
    pub kind: CommentKindArg,
    /// File path; without it, a review-level comment.
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long, value_enum, requires = "line")]
    pub side: Option<SideArg>,
    /// 1-based line on --side (default new); without it, a file comment.
    #[arg(long, requires = "path")]
    pub line: Option<u32>,
    /// First line of a range.
    #[arg(long, requires = "line")]
    pub start_line: Option<u32>,
    /// Markdown body; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    pub body_file: PathBuf,
}

/// `polygloss wait-review <review_id> [--since <seq>] [--timeout <s>]`
/// (`wait_for_review`).
#[derive(Debug, Clone, Args)]
pub struct WaitReviewArgs {
    #[arg(value_name = "REVIEW_ID")]
    pub review_id: String,
    /// Event seq to wait after (`next_since` of the previous call).
    #[arg(long, value_name = "SEQ")]
    pub since: Option<i64>,
    /// Seconds to wait (default and max 1500).
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u32>,
}

/// `polygloss rereview <review_id> --summary-file -` (`request_rereview`).
#[derive(Debug, Clone, Args)]
pub struct RereviewArgs {
    #[arg(value_name = "REVIEW_ID")]
    pub review_id: String,
    /// What changed, in markdown; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    pub summary_file: PathBuf,
}

/// `polygloss focus <diff_id|review_id> [--path … --line …]` (`focus`).
#[derive(Debug, Clone, Args)]
pub struct FocusArgs {
    /// A review id, or a diff id or prefix.
    #[arg(value_name = "DIFF_OR_REVIEW_ID")]
    pub target: String,
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long, value_enum, requires = "line")]
    pub side: Option<SideArg>,
    /// 1-based line on --side (default new).
    #[arg(long, requires = "path")]
    pub line: Option<u32>,
    /// Focus a thread instead of a line.
    #[arg(long, value_name = "THREAD_ID")]
    pub thread: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AssignedArg {
    Any,
    Me,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThreadStatusArg {
    Open,
    Resolved,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AuthorArg {
    Human,
    Agent,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThreadKindArg {
    Comment,
    Note,
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CommentKindArg {
    Note,
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SideArg {
    Old,
    New,
}

impl JsonCommand {
    /// The JSON command `command` is, or `command` back (boxed) when it is
    /// another.
    pub fn from_command(command: Command) -> Result<JsonCommand, Box<Command>> {
        Ok(match command {
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
            other => return Err(Box::new(other)),
        })
    }

    /// Whether the command writes to the store (and so needs its session row:
    /// comments reference `sessions`).
    pub fn writes(&self) -> bool {
        !matches!(
            self,
            JsonCommand::Reviews(_)
                | JsonCommand::Threads(_)
                | JsonCommand::Thread(_)
                | JsonCommand::WaitReview(_)
                | JsonCommand::Focus(_)
        )
    }
}

/// Runs one JSON command and returns the object to print (always JSON).
pub fn run(command: JsonCommand, global: &GlobalArgs) -> Result<Value, CliError> {
    let env = |k: &str| std::env::var(k).ok();
    let explicit_session = session_from(global.session.as_deref(), env);
    let ctx = ApiContext {
        core: super::core()?,
        session_id: explicit_session
            .clone()
            .unwrap_or_else(|| format!("pg-{}", polygloss_core::new_uuid())),
        client_name: agent_name(global.agent.as_deref(), env),
        launcher: if global.no_open {
            Arc::new(NoLaunch)
        } else {
            Arc::new(SystemLauncher::from_env())
        },
        roots: Vec::new(),
    };
    if command.writes() {
        record_session(&ctx)?;
    }
    match command {
        JsonCommand::Reviews(a) => {
            if a.assigned == Some(AssignedArg::Me) && explicit_session.is_none() {
                return Err(CliError::new(
                    "conflict",
                    format!("--assigned me needs --session (or ${SESSION_ENV})"),
                ));
            }
            let repo = match &global.repo {
                Some(p) => Some(path_text(p, "--repo")?),
                None => None,
            };
            let req = api::ListReviewsRequest {
                repo,
                status: a.status,
                assigned: a.assigned.map(Into::into),
                cursor: a.cursor,
                limit: a.limit,
            };
            to_json(api::list_reviews(&ctx, req))
        }
        JsonCommand::Threads(a) => {
            let req = api::ListThreadsRequest {
                review_id: Some(a.review_id),
                diff_id: None,
                status: a.status.map(Into::into),
                author: a.author.map(Into::into),
                kind: a.kind.map(Into::into),
                path: a.path,
                since: a.since,
                cursor: a.cursor,
                limit: a.limit,
            };
            to_json(api::list_threads(&ctx, req))
        }
        JsonCommand::Thread(a) => to_json(api::get_thread(
            &ctx,
            api::GetThreadRequest {
                thread_id: a.thread_id,
            },
        )),
        JsonCommand::Reply(a) => {
            let req = api::ReplyRequest {
                thread_id: a.thread_id,
                body_md: read_body(&a.body_file)?,
                resolve: a.resolve.then_some(true),
            };
            to_json(api::reply(&ctx, req))
        }
        JsonCommand::Resolve(a) => {
            let body_md = match &a.body_file {
                Some(path) => Some(read_body(path)?),
                None => None,
            };
            let req = api::ResolveRequest {
                thread_id: a.thread_id,
                body_md,
            };
            to_json(api::resolve(&ctx, req))
        }
        JsonCommand::Unresolve(a) => to_json(api::unresolve(
            &ctx,
            api::UnresolveRequest {
                thread_id: a.thread_id,
            },
        )),
        JsonCommand::Edit(a) => {
            let req = api::EditCommentRequest {
                comment_id: a.comment_id,
                body_md: read_body(&a.body_file)?,
            };
            to_json(api::edit_comment(&ctx, req))
        }
        JsonCommand::Delete(a) => to_json(api::delete_comment(
            &ctx,
            api::DeleteCommentRequest {
                comment_id: a.comment_id,
            },
        )),
        JsonCommand::Comment(a) => {
            let body_md = read_body(&a.body_file)?;
            let anchor = a.path.map(|path| AnchorParam {
                path,
                side: a.side.map(Into::into),
                line: a.line,
                start_line: a.start_line,
            });
            let req = api::CreateCommentRequest {
                review_id: Some(a.review_id),
                diff_id: None,
                kind: a.kind.into(),
                body_md,
                anchor,
            };
            to_json(api::create_comment(&ctx, req))
        }
        JsonCommand::WaitReview(a) => {
            let req = api::WaitForReviewRequest {
                review_id: a.review_id,
                since: a.since,
                timeout_s: a.timeout,
            };
            // Never cancelled: it runs until the outcome (Ctrl-C ends the process).
            to_json(api::wait_for_review(&ctx, req, &WaitControl::default()))
        }
        JsonCommand::Rereview(a) => {
            let req = api::RequestRereviewRequest {
                review_id: a.review_id,
                summary_md: read_body(&a.summary_file)?,
            };
            to_json(api::request_rereview(&ctx, req))
        }
        JsonCommand::Focus(a) => {
            let (review_id, diff_id) = focus_target(&a.target);
            let req = api::FocusRequest {
                diff_id,
                review_id,
                path: a.path,
                side: a.side.map(Into::into),
                line: a.line,
                thread_id: a.thread,
            };
            if global.no_open {
                api::focus::check(&ctx, req)?;
                return to_json(Ok(api::FocusResult {
                    status: FocusStatus::Unavailable,
                }));
            }
            to_json(api::focus(&ctx, req))
        }
    }
}

/// The author name: a non-empty `--agent`, else a non-empty
/// `$POLYGLOSS_AGENT`, else [`DEFAULT_AGENT`].
pub fn agent_name(flag: Option<&str>, env: impl Fn(&str) -> Option<String>) -> String {
    non_empty(flag.map(str::to_owned))
        .or_else(|| non_empty(env(AGENT_ENV)))
        .unwrap_or_else(|| DEFAULT_AGENT.to_owned())
}

/// The caller's session when it named one: a non-empty `--session`, else a
/// non-empty `$CLAUDE_CODE_SESSION_ID`.
pub fn session_from(flag: Option<&str>, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    non_empty(flag.map(str::to_owned)).or_else(|| non_empty(env(SESSION_ENV)))
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// `focus`'s positional target as `(review_id, diff_id)`: all hex digits is a
/// diff id or prefix, anything else a review id.
pub fn focus_target(target: &str) -> (Option<String>, Option<String>) {
    let t = target.trim();
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        (None, Some(t.to_owned()))
    } else {
        (Some(t.to_owned()), None)
    }
}

/// Upserts the caller's session so writes can reference it: an existing row
/// is kept as it is (client, version, owner pid); a new one is named after the
/// author, without an owner pid.
fn record_session(ctx: &ApiContext) -> Result<(), CliError> {
    let info = match ctx.core.session(&ctx.session_id)? {
        Some(existing) => SessionInfo {
            cwd: None,
            ..existing
        },
        None => SessionInfo {
            id: ctx.session_id.clone(),
            client_name: ctx.client_name.clone(),
            client_version: None,
            owner_pid: None,
            cwd: std::env::current_dir().ok(),
        },
    };
    ctx.core.upsert_session(&info)?;
    Ok(())
}

/// A body from `path` (`-` = stdin).
fn read_body(path: &Path) -> Result<String, CliError> {
    let flag_path = path.display();
    let bytes = if path == Path::new("-") {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| CliError::internal(format!("reading stdin: {e}")))?;
        buf
    } else {
        std::fs::read(path).map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                "not_found"
            } else {
                "internal"
            };
            CliError::new(code, format!("cannot read {flag_path}: {e}"))
        })?
    };
    body_text(bytes, &flag_path.to_string())
}

/// `bytes` as UTF-8 without trailing newlines (`conflict` when not UTF-8).
pub fn body_text(bytes: Vec<u8>, source: &str) -> Result<String, CliError> {
    let text = String::from_utf8(bytes)
        .map_err(|_| CliError::new("conflict", format!("{source} is not UTF-8 text")))?;
    Ok(text.trim_end_matches(['\n', '\r']).to_owned())
}

fn path_text(path: &Path, flag: &str) -> Result<String, CliError> {
    let abs = std::path::absolute(path)
        .map_err(|e| CliError::internal(format!("cannot resolve {}: {e}", path.display())))?;
    abs.to_str()
        .map(str::to_owned)
        .ok_or_else(|| CliError::new("conflict", format!("{flag} is not a UTF-8 path")))
}

/// An api result as the JSON its MCP twin puts in `structuredContent`.
fn to_json<T: Serialize>(result: Result<T, polygloss_mcp::ApiError>) -> Result<Value, CliError> {
    let value = result?;
    serde_json::to_value(value).map_err(|e| CliError::internal(format!("encoding the result: {e}")))
}

/// The launcher under `--no-open`: refuses, so nothing ever starts the app.
struct NoLaunch;

impl Launcher for NoLaunch {
    fn launch(&self, _url: Option<&str>, _activate: bool) -> std::io::Result<()> {
        Err(std::io::Error::other("--no-open: not launching Polygloss"))
    }
}

impl From<AssignedArg> for AssignedFilter {
    fn from(a: AssignedArg) -> AssignedFilter {
        match a {
            AssignedArg::Any => AssignedFilter::Any,
            AssignedArg::Me => AssignedFilter::Me,
        }
    }
}

impl From<ThreadStatusArg> for ThreadStatusFilter {
    fn from(a: ThreadStatusArg) -> ThreadStatusFilter {
        match a {
            ThreadStatusArg::Open => ThreadStatusFilter::Open,
            ThreadStatusArg::Resolved => ThreadStatusFilter::Resolved,
            ThreadStatusArg::All => ThreadStatusFilter::All,
        }
    }
}

impl From<AuthorArg> for AuthorFilter {
    fn from(a: AuthorArg) -> AuthorFilter {
        match a {
            AuthorArg::Human => AuthorFilter::Human,
            AuthorArg::Agent => AuthorFilter::Agent,
            AuthorArg::Any => AuthorFilter::Any,
        }
    }
}

impl From<ThreadKindArg> for ThreadKindParam {
    fn from(a: ThreadKindArg) -> ThreadKindParam {
        match a {
            ThreadKindArg::Comment => ThreadKindParam::Comment,
            ThreadKindArg::Note => ThreadKindParam::Note,
            ThreadKindArg::Question => ThreadKindParam::Question,
        }
    }
}

impl From<CommentKindArg> for AgentThreadKind {
    fn from(a: CommentKindArg) -> AgentThreadKind {
        match a {
            CommentKindArg::Note => AgentThreadKind::Note,
            CommentKindArg::Question => AgentThreadKind::Question,
        }
    }
}

impl From<SideArg> for SideParam {
    fn from(a: SideArg) -> SideParam {
        match a {
            SideArg::Old => SideParam::Old,
            SideArg::New => SideParam::New,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn agent_name_prefers_flag_then_env_then_default() {
        let env = env_with(&[(AGENT_ENV, "cursor")]);
        assert_eq!(agent_name(Some("codex"), &env), "codex");
        assert_eq!(agent_name(None, &env), "cursor");
        assert_eq!(agent_name(Some("  "), &env), "cursor");
        assert_eq!(agent_name(None, env_with(&[(AGENT_ENV, "")])), "agent");
        assert_eq!(agent_name(None, env_with(&[])), DEFAULT_AGENT);
    }

    #[test]
    fn session_prefers_flag_then_env() {
        let env = env_with(&[(SESSION_ENV, "from-env")]);
        assert_eq!(session_from(Some("s1"), &env).as_deref(), Some("s1"));
        assert_eq!(session_from(None, &env).as_deref(), Some("from-env"));
        assert_eq!(session_from(Some(""), &env).as_deref(), Some("from-env"));
        assert_eq!(session_from(None, env_with(&[(SESSION_ENV, " ")])), None);
        assert_eq!(session_from(None, env_with(&[])), None);
    }

    #[test]
    fn focus_target_splits_diff_ids_from_review_ids() {
        assert_eq!(
            focus_target("0123abcd"),
            (None, Some("0123abcd".to_owned()))
        );
        assert_eq!(
            focus_target("01a0f343-5fea-72ab-b474-191e936642cf"),
            (
                Some("01a0f343-5fea-72ab-b474-191e936642cf".to_owned()),
                None
            )
        );
        assert_eq!(focus_target(""), (Some(String::new()), None));
    }

    #[test]
    fn bodies_are_utf8_without_trailing_newlines() {
        assert_eq!(body_text(b"Hi.\n\n".to_vec(), "-").expect("text"), "Hi.");
        assert_eq!(
            body_text(b"a\r\nb\r\n".to_vec(), "-").expect("text"),
            "a\r\nb"
        );
        assert_eq!(
            body_text(b"  keep  ".to_vec(), "-").expect("text"),
            "  keep  "
        );
        let err = body_text(vec![0xff, 0xfe], "body.md").expect_err("not utf-8");
        assert_eq!(err.code, "conflict");
        assert!(err.message.contains("body.md"), "{}", err.message);
    }

    #[test]
    fn only_store_writes_record_the_session() {
        use clap::Parser as _;
        let parse = |argv: &[&str]| match crate::cli::Cli::try_parse_from(argv)
            .expect("parses")
            .command
        {
            Some(c) => JsonCommand::from_command(c).expect("a json command"),
            None => panic!("no subcommand"),
        };
        for argv in [
            ["polygloss", "reply", "t", "--body-file", "-"].as_slice(),
            &["polygloss", "resolve", "t"],
            &["polygloss", "unresolve", "t"],
            &["polygloss", "edit", "c", "--body-file", "-"],
            &["polygloss", "delete", "c"],
            &[
                "polygloss",
                "comment",
                "r",
                "--kind",
                "note",
                "--body-file",
                "-",
            ],
            &["polygloss", "rereview", "r", "--summary-file", "-"],
        ] {
            assert!(parse(argv).writes(), "{argv:?}");
        }
        for argv in [
            ["polygloss", "reviews"].as_slice(),
            &["polygloss", "threads", "r"],
            &["polygloss", "thread", "t"],
            &["polygloss", "wait-review", "r"],
            &["polygloss", "focus", "r"],
        ] {
            assert!(!parse(argv).writes(), "{argv:?}");
        }
    }

    #[test]
    fn no_open_launcher_refuses() {
        assert!(
            NoLaunch
                .launch(Some("polygloss://review/x"), false)
                .is_err()
        );
    }
}
