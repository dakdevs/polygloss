//! Hidden `polygloss debug seed|human-comment|human-viewed|human-submit`: the
//! human's side of a review for bun tests (T4.4, OQ-P4). They run only with
//! `POLYGLOSS_TEST=1` and print one JSON object on stdout.
//!
//! - `seed` opens a review the way the app would (live states are pinned, as a
//!   human comment would pin them) and prints its ids.
//! - `human-comment` adds a human draft thread (or a draft reply with
//!   `--reply-to`) on the review's latest iteration; drafts stay invisible to
//!   agents until `human-submit`.
//! - `human-viewed` marks a file of the latest iteration viewed.
//! - `human-submit` publishes the drafts with a verdict and summary.

use std::path::PathBuf;

use anyhow::{Context as _, anyhow, bail};
use clap::{Args, ValueEnum};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, PinnedBy, Subject, ThreadKind, Verdict,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;
use serde_json::{Value, json};

/// The variable that enables test-only surfaces (OQ-P4).
pub const TEST_ENV: &str = "POLYGLOSS_TEST";

/// `debug seed`: open a review and print `{review_id, review_key, diff_id, iteration}`.
#[derive(Debug, Args)]
pub struct SeedArgs {
    /// Any path inside the worktree or repository.
    #[arg(long)]
    pub repo: PathBuf,
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

pub fn seed(args: SeedArgs) -> anyhow::Result<Value> {
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
        worktree: args.repo,
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
    let latest = core
        .iterations(&args.review)?
        .pop()
        .ok_or_else(|| anyhow!("review {} has no iteration", args.review))?;
    let subject = match (args.path, args.line) {
        (Some(path), Some(line)) => Subject::Line {
            path,
            side: match args.side {
                Some(SideArg::Old) => Side::Old,
                _ => Side::New,
            },
            start_line: args.start_line.unwrap_or(line),
            line,
        },
        (Some(path), None) => Subject::File { path },
        (None, _) => Subject::Review,
    };
    let (repo, _) = core.find_repo_for_diff(latest.diff_id.as_str(), None)?;
    let blobs = BlobReader::open(&repo)?;
    let thread_id = core.create_thread(
        &NewThread {
            review_id: args.review,
            diff_id: latest.diff_id.clone(),
            subject,
            kind: ThreadKind::Comment,
            body_md: args.body,
            author: human(),
        },
        &blobs,
    )?;
    Ok(json!({ "thread_id": thread_id, "diff_id": latest.diff_id.as_str() }))
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
