//! The JSON CLI for agents without MCP (design §14 "JSON CLI", T4.9): one
//! command per MCP tool, printing the same JSON as its `polygloss_mcp::api`
//! twin. T4.3 defines the arguments; T4.9 implements [`run`].
//!
//! Bodies are read from `--body-file` / `--summary-file` (`-` = stdin). The
//! author name is `--agent`, else `$POLYGLOSS_AGENT`, else `agent`; the
//! session is `--session`.

use std::path::PathBuf;

use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::cli::GlobalArgs;
use crate::output::CliError;

/// One JSON CLI command with its arguments.
#[derive(Debug, Clone)]
#[expect(
    dead_code,
    reason = "T4.9 implements the handlers that read the arguments"
)]
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

impl JsonCommand {
    /// The command name on the command line.
    pub fn name(&self) -> &'static str {
        match self {
            JsonCommand::Reviews(_) => "reviews",
            JsonCommand::Threads(_) => "threads",
            JsonCommand::Thread(_) => "thread",
            JsonCommand::Reply(_) => "reply",
            JsonCommand::Resolve(_) => "resolve",
            JsonCommand::Unresolve(_) => "unresolve",
            JsonCommand::Edit(_) => "edit",
            JsonCommand::Delete(_) => "delete",
            JsonCommand::Comment(_) => "comment",
            JsonCommand::WaitReview(_) => "wait-review",
            JsonCommand::Rereview(_) => "rereview",
            JsonCommand::Focus(_) => "focus",
        }
    }
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

/// Runs one JSON command and returns the object to print (always JSON).
pub fn run(command: JsonCommand, _global: &GlobalArgs) -> Result<Value, CliError> {
    Err(CliError::internal(format!(
        "`polygloss {}` is not implemented in this build",
        command.name()
    )))
}
