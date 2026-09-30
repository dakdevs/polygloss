//! Threads as agents see them (design §15.2 `ThreadSummary`, §8.6): positions
//! in the diff a list is relative to, and the summary fields shared by
//! `list_threads`, `get_thread`, the thread resources and `wait_for_review`.
//!
//! Positions are relative to the review's latest iteration, or to the diff the
//! caller names; a thread of a review without iterations falls back to its
//! origin diff. They come from core's carry-forward cache (`thread_positions`)
//! and are computed from the repo's objects on a miss.

use std::collections::HashMap;
use std::sync::Arc;

use polygloss_core::DiffId;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AuthorKind, CommentView, Position, PositionState, Subject, ThreadView,
};
use polygloss_diff::FileChange;

use crate::api::shapes::{
    CreatedBy, LastComment, PositionOut, SideParam, ThreadSummary, excerpt, timestamp,
};
use crate::context::ApiContext;
use crate::errors::ApiError;

/// A stored diff with its file list and a reader for its repo's objects.
#[derive(Debug, Clone)]
pub struct DiffContext {
    pub diff_id: DiffId,
    pub files: Arc<Vec<FileChange>>,
    pub blobs: BlobReader,
}

impl DiffContext {
    /// Loads a stored diff: its files, and the objects of a repo that has both
    /// of its trees (`repo_not_found` when none has).
    pub fn load(ctx: &ApiContext, diff_id: &DiffId) -> Result<DiffContext, ApiError> {
        let files = ctx
            .core
            .files_for_diff(diff_id)?
            .ok_or_else(|| ApiError::not_found(format!("diff not found: {diff_id}")))?;
        let (repo, _) = ctx.core.find_repo_for_diff(diff_id.as_str(), None)?;
        let blobs = BlobReader::open(&repo)
            .map_err(|e| ApiError::from(polygloss_core::review::CoreError::from(e)))?;
        Ok(DiffContext {
            diff_id: diff_id.clone(),
            files,
            blobs,
        })
    }

    /// The file shown at `path` (its display path) in this diff.
    pub fn file(&self, path: &str) -> Option<&FileChange> {
        self.files.iter().find(|f| f.display_path() == path)
    }
}

/// The diff positions are relative to by default: the review's latest
/// iteration (`None` for a review without one, or no review).
pub fn latest_diff(ctx: &ApiContext, review_id: Option<&str>) -> Result<Option<DiffId>, ApiError> {
    let Some(review) = review_id else {
        return Ok(None);
    };
    Ok(ctx.core.iterations(review)?.pop().map(|it| it.diff_id))
}

/// Thread positions keyed by thread id, and the diffs loaded to place them.
pub type Placed = (HashMap<String, Position>, HashMap<DiffId, DiffContext>);

/// Positions of `threads` in `target` (each thread's origin diff when `None`),
/// keyed by thread id, plus the loaded diffs keyed by id.
pub fn positions(
    ctx: &ApiContext,
    threads: &[&ThreadView],
    target: Option<&DiffId>,
) -> Result<Placed, ApiError> {
    let mut by_diff: Vec<(DiffId, Vec<String>)> = Vec::new();
    for t in threads {
        let diff = target.unwrap_or(&t.origin_diff_id);
        match by_diff.iter_mut().find(|(d, _)| d == diff) {
            Some((_, ids)) => ids.push(t.id.clone()),
            None => by_diff.push((diff.clone(), vec![t.id.clone()])),
        }
    }
    let mut out = HashMap::new();
    let mut diffs = HashMap::new();
    for (diff, ids) in by_diff {
        let dc = DiffContext::load(ctx, &diff)?;
        out.extend(ctx.core.positions(&diff, &dc.files, &ids, &dc.blobs)?);
        diffs.insert(diff, dc);
    }
    Ok((out, diffs))
}

/// The comments that count (a deleted root's placeholder does not).
pub fn live_comments(t: &ThreadView) -> impl Iterator<Item = &CommentView> {
    t.comments.iter().filter(|c| !c.deleted)
}

/// Whether a thread's suggestion blocks apply: a line thread on the new side
/// (design §8.5; elsewhere they are plain code blocks).
pub fn takes_suggestions(t: &ThreadView) -> bool {
    matches!(
        t.anchor.subject,
        Subject::Line {
            side: polygloss_diff::Side::New,
            ..
        }
    )
}

/// The position as agents see it: `path` only when it differs from the anchor.
pub fn position_out(t: &ThreadView, p: Option<&Position>) -> PositionOut {
    let Some(p) = p else {
        // A thread deleted between listing and placing: nowhere in this diff.
        return PositionOut {
            state: PositionState::Absent,
            path: None,
            start_line: None,
            line: None,
        };
    };
    let path = p
        .path
        .clone()
        .filter(|path| Some(path.as_str()) != t.anchor.subject.path());
    PositionOut {
        state: p.state,
        path,
        start_line: p.start_line,
        line: p.line,
    }
}

/// The summary of a thread at position `p`.
pub fn summarize(t: &ThreadView, p: Option<&Position>) -> ThreadSummary {
    let (path, side, start_line, line) = match &t.anchor.subject {
        Subject::Line {
            path,
            side,
            start_line,
            line,
        } => (
            Some(path.clone()),
            Some(SideParam::from(*side)),
            Some(*start_line),
            Some(*line),
        ),
        Subject::File { path } => (Some(path.clone()), None, None, None),
        Subject::Review => (None, None, None, None),
    };
    let live: Vec<&CommentView> = live_comments(t).collect();
    let has_suggestion = takes_suggestions(t)
        && live
            .iter()
            .any(|c| !polygloss_core::review::parse_suggestions(&c.body_md).is_empty());
    let last_comment = match live.last() {
        Some(c) => LastComment {
            author_kind: c.author.kind,
            author_name: c.author.name.clone(),
            excerpt: excerpt(&c.body_md),
            at: timestamp(c.published_at.unwrap_or(c.created_at)),
        },
        // Core never returns a thread without a live comment to an agent.
        None => LastComment {
            author_kind: AuthorKind::Human,
            author_name: String::new(),
            excerpt: String::new(),
            at: timestamp(t.updated_at),
        },
    };
    ThreadSummary {
        thread_id: t.id.clone(),
        review_id: t.review_id.clone(),
        kind: t.kind,
        subject: t.anchor.subject.as_str(),
        path,
        side,
        start_line,
        line,
        position: position_out(t, p),
        status: t.status,
        created_by: CreatedBy {
            kind: t.created_by.kind,
            name: t.created_by.name.clone(),
        },
        comment_count: u32::try_from(live.len()).unwrap_or(u32::MAX),
        has_suggestion,
        last_comment,
        updated_at: timestamp(t.updated_at),
    }
}
