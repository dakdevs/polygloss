//! Hidden `polygloss debug seed|human-comment|human-viewed|human-submit`: the
//! human's side of a review for bun tests (T4.4, OQ-P4), plus
//! `agent-comment|assign`, stand-ins for an agent's writes that do not go
//! through MCP (T4.5). They run only with `POLYGLOSS_TEST=1` and print one JSON
//! object on stdout.
//!
//! - `seed` opens a review the way the app would (live states are pinned, as a
//!   human comment would pin them) and prints its ids.
//! - `human-comment` adds a human draft thread (or a draft reply with
//!   `--reply-to`) on the review's latest iteration; drafts stay invisible to
//!   agents until `human-submit`.
//! - `human-viewed` marks a file of the latest iteration viewed.
//! - `human-submit` publishes the drafts with a verdict and summary.
//! - `agent-comment` adds a published agent thread (note, question or comment)
//!   on the latest iteration, or an agent reply with `--reply-to`; the author
//!   name is the global `--agent` (default `claude-code`) and the session the
//!   global `--session` (recorded on the comment, not upserted).
//! - `assign` records an agent session (the global `--session`, required) and
//!   assigns a review to it, as an agent's `open_diff` (or the human's "Assign
//!   to session…", OQ-32) would. The session has no owner pid unless
//!   `--owner-pid` is given, so sessions made by one test runner never link to
//!   each other (T4.5 read-tool and T4.8 `polygloss wait` tests).

use std::path::PathBuf;

use anyhow::{Context as _, anyhow, bail};
use clap::{Args, ValueEnum};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AssignedBy, Author, AuthorKind, Core, NewThread, OpenRequest, PinnedBy, SessionInfo, Subject,
    ThreadKind, Verdict,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;
use serde_json::{Value, json};

/// The variable that enables test-only surfaces (OQ-P4).
pub const TEST_ENV: &str = "POLYGLOSS_TEST";

/// `debug seed`: open a review and print `{review_id, review_key, diff_id, iteration}`.
/// The worktree or repository is the global `--repo` (required).
#[derive(Debug, Args)]
pub struct SeedArgs {
    /// Review one commit against its first parent.
    #[arg(long, conflicts_with_all = ["base", "head"])]
    pub commit: Option<String>,
    /// Compare base (needs --head).
    #[arg(long, requires = "head")]
    pub base: Option<String>,
    /// Compare head (needs --base).
    #[arg(long, requires = "base")]
    pub head: Option<String>,
    /// Two-dot compare instead of three-dot.
    #[arg(long, requires = "base")]
    pub direct: bool,
    /// Live: `merge-base` (default), `HEAD` or a revision.
    #[arg(long, conflicts_with_all = ["commit", "base"])]
    pub since: Option<String>,
    #[arg(long)]
    pub label: Option<String>,
}

/// `debug human-comment`: a human draft thread or reply.
#[derive(Debug, Args)]
pub struct HumanCommentArgs {
    #[arg(long)]
    pub review: String,
    /// Markdown body.
    #[arg(long)]
    pub body: String,
    /// Reply to this thread instead of starting one.
    #[arg(long, conflicts_with_all = ["path", "side", "line", "start_line"])]
    pub reply_to: Option<String>,
    /// File path; without --line a file thread, without --path a review thread.
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long, value_enum, requires = "line")]
    pub side: Option<SideArg>,
    #[arg(long, requires = "path")]
    pub line: Option<u32>,
    #[arg(long, requires = "line")]
    pub start_line: Option<u32>,
}

/// `debug human-viewed`: mark a file of the latest iteration viewed.
#[derive(Debug, Args)]
pub struct HumanViewedArgs {
    #[arg(long)]
    pub review: String,
    #[arg(long)]
    pub path: String,
    /// Unmark instead.
    #[arg(long)]
    pub unset: bool,
}

/// `debug human-submit`: submit the review.
#[derive(Debug, Args)]
pub struct HumanSubmitArgs {
    #[arg(long)]
    pub review: String,
    #[arg(long, value_enum, default_value_t = VerdictArg::RequestChanges)]
    pub verdict: VerdictArg,
    #[arg(long, default_value = "")]
    pub summary: String,
}

/// `debug agent-comment`: a published agent thread or reply. The author is the
/// global `--agent` (default `claude-code`) and the global `--session`.
#[derive(Debug, Args)]
pub struct AgentCommentArgs {
    /// The review (not needed with --reply-to).
    #[arg(long, required_unless_present = "reply_to")]
    pub review: Option<String>,
    #[arg(long, value_enum, default_value_t = KindArg::Note)]
    pub kind: KindArg,
    /// Markdown body.
    #[arg(long)]
    pub body: String,
    /// Reply to this thread instead of starting one.
    #[arg(long, conflicts_with_all = ["review", "path", "side", "line", "start_line"])]
    pub reply_to: Option<String>,
    /// File path; without --line a file thread, without --path a review thread.
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long, value_enum, requires = "line")]
    pub side: Option<SideArg>,
    #[arg(long, requires = "path")]
    pub line: Option<u32>,
    #[arg(long, requires = "line")]
    pub start_line: Option<u32>,
}

/// `debug assign`: record a session (the global `--session`) and assign the
/// review to it.
#[derive(Debug, Args)]
pub struct AssignArgs {
    #[arg(long)]
    pub review: String,
    /// The session's client name (MCP `clientInfo.name`).
    #[arg(long, default_value = "claude-code")]
    pub client: String,
    /// The agent host process that owns the session (links drifted ids, §16.4).
    #[arg(long)]
    pub owner_pid: Option<i32>,
    #[arg(long, value_enum, default_value_t = AssignedByArg::OpenDiff)]
    pub by: AssignedByArg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AssignedByArg {
    OpenDiff,
    Human,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum KindArg {
    Note,
    Question,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SideArg {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum VerdictArg {
    RequestChanges,
    Comment,
    Approve,
}

/// Fails unless `POLYGLOSS_TEST=1`.
pub fn require_test_env(command: &str) -> anyhow::Result<()> {
    if std::env::var_os(TEST_ENV).is_some_and(|v| v == "1") {
        Ok(())
    } else {
        bail!("`debug {command}` is a test-only command; set {TEST_ENV}=1")
    }
}

fn core() -> anyhow::Result<Core> {
    Core::open_default().context("opening the Polygloss store")
}

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

pub fn seed(repo: PathBuf, args: SeedArgs) -> anyhow::Result<Value> {
    require_test_env("seed")?;
    let core = core()?;
    let source = match (&args.commit, &args.base, &args.head) {
        (Some(rev), _, _) => Source::Commit { rev: rev.clone() },
        (None, Some(base), Some(head)) => Source::Compare {
            base: base.clone(),
            head: head.clone(),
            mode: if args.direct {
                CompareMode::Direct
            } else {
                CompareMode::ThreeDot
            },
        },
        _ => Source::Live {
            since: match args.since.as_deref() {
                None | Some("merge-base") => Since::MergeBase,
                Some("HEAD") => Since::Head,
                Some(rev) => Since::Commit(rev.to_owned()),
            },
        },
    };
    let live = matches!(source, Source::Live { .. });
    let opened = core.open(&OpenRequest {
        worktree: repo,
        source,
        label: args.label,
        pin: live.then_some(PinnedBy::Comment),
        actor: Actor::human(),
    })?;
    let iteration = opened
        .iteration
        .as_ref()
        .ok_or_else(|| anyhow!("the open recorded no iteration"))?;
    Ok(json!({
        "review_id": opened.review_id,
        "review_key": opened.review_key,
        "diff_id": opened.diff_id.as_str(),
        "iteration": iteration.seq,
        "files": opened.files.len(),
    }))
}

pub fn human_comment(args: HumanCommentArgs) -> anyhow::Result<Value> {
    require_test_env("human-comment")?;
    let core = core()?;
    if let Some(thread_id) = &args.reply_to {
        let comment_id = core.reply(thread_id, &args.body, &human())?;
        return Ok(json!({ "thread_id": thread_id, "comment_id": comment_id }));
    }
    let subject = subject(args.path, args.side, args.line, args.start_line);
    new_thread(
        &core,
        args.review,
        subject,
        ThreadKind::Comment,
        args.body,
        human(),
    )
}

/// The subject named by `--path`, `--side`, `--line` and `--start-line`.
fn subject(
    path: Option<String>,
    side: Option<SideArg>,
    line: Option<u32>,
    start_line: Option<u32>,
) -> Subject {
    match (path, line) {
        (Some(path), Some(line)) => Subject::Line {
            path,
            side: match side {
                Some(SideArg::Old) => Side::Old,
                _ => Side::New,
            },
            start_line: start_line.unwrap_or(line),
            line,
        },
        (Some(path), None) => Subject::File { path },
        (None, _) => Subject::Review,
    }
}

/// Creates a thread on the review's latest iteration and prints its ids.
fn new_thread(
    core: &Core,
    review_id: String,
    subject: Subject,
    kind: ThreadKind,
    body_md: String,
    author: Author,
) -> anyhow::Result<Value> {
    let latest = core
        .iterations(&review_id)?
        .pop()
        .ok_or_else(|| anyhow!("review {review_id} has no iteration"))?;
    let (repo, _) = core.find_repo_for_diff(latest.diff_id.as_str(), None)?;
    let blobs = BlobReader::open(&repo)?;
    let thread_id = core.create_thread(
        &NewThread {
            review_id,
            diff_id: latest.diff_id.clone(),
            subject,
            kind,
            body_md,
            author,
        },
        &blobs,
    )?;
    Ok(json!({ "thread_id": thread_id, "diff_id": latest.diff_id.as_str() }))
}

/// Default author name of `agent-comment` without the global `--agent`.
pub const DEFAULT_AGENT: &str = "claude-code";

pub fn agent_comment(
    args: AgentCommentArgs,
    agent: Option<String>,
    session: Option<String>,
) -> anyhow::Result<Value> {
    require_test_env("agent-comment")?;
    let core = core()?;
    let author = Author {
        kind: AuthorKind::Agent,
        name: agent.unwrap_or_else(|| DEFAULT_AGENT.to_owned()),
        session_id: session,
    };
    if let Some(thread_id) = &args.reply_to {
        let comment_id = core.reply(thread_id, &args.body, &author)?;
        return Ok(json!({ "thread_id": thread_id, "comment_id": comment_id }));
    }
    let review = args
        .review
        .ok_or_else(|| anyhow!("--review or --reply-to is required"))?;
    let kind = match args.kind {
        KindArg::Note => ThreadKind::Note,
        KindArg::Question => ThreadKind::Question,
        KindArg::Comment => ThreadKind::Comment,
    };
    let subject = subject(args.path, args.side, args.line, args.start_line);
    new_thread(&core, review, subject, kind, args.body, author)
}

pub fn assign(args: AssignArgs, session: Option<String>) -> anyhow::Result<Value> {
    require_test_env("assign")?;
    let session = session.ok_or_else(|| anyhow!("--session <ID> is required"))?;
    let core = core()?;
    let canonical = core.upsert_session(&SessionInfo {
        id: session.clone(),
        client_name: args.client,
        client_version: None,
        owner_pid: args.owner_pid,
        cwd: None,
    })?;
    let by = match args.by {
        AssignedByArg::OpenDiff => AssignedBy::OpenDiff,
        AssignedByArg::Human => AssignedBy::Human,
        AssignedByArg::Agent => AssignedBy::Agent,
    };
    core.assign_review(&args.review, &session, by)?;
    Ok(json!({
        "review_id": args.review,
        "session_id": session,
        "canonical_id": canonical,
        "assigned_by": by.as_str(),
    }))
}

pub fn human_viewed(args: HumanViewedArgs) -> anyhow::Result<Value> {
    require_test_env("human-viewed")?;
    let core = core()?;
    let latest = core
        .iterations(&args.review)?
        .pop()
        .ok_or_else(|| anyhow!("review {} has no iteration", args.review))?;
    let files = core
        .files_for_diff(&latest.diff_id)?
        .ok_or_else(|| anyhow!("diff {} is not stored", latest.diff_id.as_str()))?;
    let change = files
        .iter()
        .find(|f| f.display_path() == args.path)
        .ok_or_else(|| anyhow!("{} is not in the diff", args.path))?;
    core.set_viewed(Some(&args.review), change, !args.unset)?;
    Ok(json!({ "path": args.path, "viewed": !args.unset }))
}

pub fn human_submit(args: HumanSubmitArgs) -> anyhow::Result<Value> {
    require_test_env("human-submit")?;
    let core = core()?;
    let verdict = match args.verdict {
        VerdictArg::RequestChanges => Verdict::RequestChanges,
        VerdictArg::Comment => Verdict::Comment,
        VerdictArg::Approve => Verdict::Approve,
    };
    let s = core.submit_review(&args.review, verdict, &args.summary, None)?;
    Ok(json!({
        "submission_id": s.id,
        "review_id": s.review_id,
        "verdict": s.verdict.as_str(),
        "iteration": s.iteration.seq,
        "comment_count": s.comment_count,
        "seq": s.seq,
    }))
}
