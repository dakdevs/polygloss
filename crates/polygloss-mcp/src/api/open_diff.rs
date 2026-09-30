//! `open_diff` (design §15.2): resolve the source, pin a live state
//! (`pinned_by = agent`), record the review and iteration, assign the review to
//! the caller (latest opener wins) and ask the app to show it in the background.
//!
//! - Live sources are pinned by the open itself (`OpenRequest.pin = Agent`), so
//!   the result always names a stored iteration and `url` is the diff's.
//!   Commit and compare opens record a new iteration only when the trees moved.
//! - `stats.additions`/`deletions` and the per-file counts cover the listed
//!   files (the first [`LISTED_FILES`] in `diff-tree` order); binaries and
//!   submodules have no counts.
//! - `app` is `skipped` with `show: false`; an app that cannot be reached is
//!   `unavailable`, never an error.

use polygloss_core::git::ResolvedSide;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{AssignedBy, OpenRequest, PinnedBy};
use polygloss_core::urls::{PolyglossUrl, format_url};
use polygloss_diff::{FileChange, FileStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::resources::{file_status, line_counts_with};
use crate::api::shapes::SourceParam;
use crate::app_link::{self, AppShown};
use crate::context::ApiContext;
use crate::errors::ApiError;

/// How many files `open_diff` lists (and counts lines of).
pub const LISTED_FILES: usize = 200;

/// `open_diff` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenDiffRequest {
    /// Any path inside the repository. Default: the client's first root that is a
    /// git worktree, else the project directory, else the server's cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// What to diff. Default: `{kind: "live"}` (working tree vs merge-base).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceParam>,
    /// Display label such as `PR #123`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Open the review in the Polygloss app (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
    /// Assign the review to this session so its submission wakes you (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign: Option<bool>,
}

/// One side of the diff: the ref it was resolved from (when it was a ref), the
/// commit and the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffSide {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub tree: String,
}

/// Totals: every file, and the lines of the listed ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DiffStats {
    pub files: usize,
    pub additions: u32,
    pub deletions: u32,
}

/// One listed file. `path` is where it is shown (the new path, or the old one
/// of a deleted file); `old_path` only for renames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// `added`, `modified`, `deleted`, `renamed` or `type_changed`.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additions: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u32>,
}

/// `open_diff` result (design §15.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenDiffResult {
    pub review_id: String,
    pub review_key: String,
    /// The iteration's `seq` (1-based within the review).
    pub iteration: u32,
    pub diff_id: String,
    /// `polygloss://diff/<diff_id>`.
    pub url: String,
    pub base: DiffSide,
    pub head: DiffSide,
    pub stats: DiffStats,
    pub files: Vec<DiffFile>,
    /// More than [`LISTED_FILES`] files changed.
    pub files_truncated: bool,
    pub app: AppShown,
    /// Resolution notices (no default branch, no merge base, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Resolves the source, pins a live state, records the review and iteration,
/// assigns it to the caller and asks the app to show it.
pub fn open_diff(ctx: &ApiContext, req: OpenDiffRequest) -> Result<OpenDiffResult, ApiError> {
    let worktree = ctx.default_repo(req.repo.as_deref());
    let source = req.source.unwrap_or_default();
    let live = matches!(source, SourceParam::Live { .. });
    let label = req
        .label
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty());
    let opened = ctx.core.open(&OpenRequest {
        worktree,
        source: source.to_core(),
        label,
        pin: live.then_some(PinnedBy::Agent),
        actor: ctx.actor(),
    })?;
    let iteration = opened
        .iteration
        .as_ref()
        .ok_or_else(|| ApiError::internal("the open recorded no iteration"))?;
    if req.assign.unwrap_or(true) {
        ctx.core
            .assign_review(&opened.review_id, &ctx.session_id, AssignedBy::OpenDiff)?;
    }
    app_link::after_write(ctx);
    let app = if req.show.unwrap_or(true) {
        app_link::show_review(ctx, &opened.review_id)
    } else {
        AppShown::Skipped
    };

    // Counts need the objects; without them the list still has every file.
    let blobs = BlobReader::open(&opened.repo).ok();
    let listed: Vec<DiffFile> = opened
        .files
        .iter()
        .take(LISTED_FILES)
        .map(|f| listed_file(f, blobs.as_ref()))
        .collect();
    let stats = DiffStats {
        files: opened.files.len(),
        additions: listed.iter().filter_map(|f| f.additions).sum(),
        deletions: listed.iter().filter_map(|f| f.deletions).sum(),
    };
    let url = format_url(&PolyglossUrl::Diff {
        diff_id: opened.diff_id.as_str().to_owned(),
        path: None,
        side: None,
        line: None,
    });
    Ok(OpenDiffResult {
        review_id: opened.review_id.clone(),
        review_key: opened.review_key.clone(),
        iteration: iteration.seq,
        diff_id: opened.diff_id.as_str().to_owned(),
        url,
        base: side(&opened.base),
        head: DiffSide {
            rev: opened.head_ref.clone(),
            commit: opened.head_commit.as_ref().map(|c| c.as_str().to_owned()),
            tree: opened.head_tree.as_str().to_owned(),
        },
        stats,
        files_truncated: opened.files.len() > LISTED_FILES,
        files: listed,
        app,
        warnings: opened.warnings.iter().map(ToString::to_string).collect(),
    })
}

fn side(s: &ResolvedSide) -> DiffSide {
    DiffSide {
        rev: s.ref_name.clone(),
        commit: s.commit.as_ref().map(|c| c.as_str().to_owned()),
        tree: s.tree.as_str().to_owned(),
    }
}

fn listed_file(f: &FileChange, blobs: Option<&BlobReader>) -> DiffFile {
    let counts = blobs.and_then(|b| line_counts_with(f, b));
    DiffFile {
        path: f.display_path().to_owned(),
        old_path: match (&f.old_path, f.status) {
            (Some(old), FileStatus::Renamed) => Some(old.text.clone()),
            _ => None,
        },
        status: file_status(f.status),
        additions: counts.map(|c| c.0),
        deletions: counts.map(|c| c.1),
    }
}
