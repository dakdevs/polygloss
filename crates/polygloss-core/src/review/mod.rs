//! The review domain: reviews, iterations, threads, drafts, submissions, Viewed,
//! view state, sessions and carry-forward (T1.12–T1.15). Converts the 0-based lines
//! of `polygloss-diff` to the 1-based anchors of the store and agent surfaces.

pub mod carry_forward;
pub mod models;
pub mod open;
pub mod sessions;
pub mod submit;
pub mod suggestions;
pub mod summary;
pub mod threads;
pub mod view_state;
pub mod viewed;

pub use models::{IterationInfo, OpenRequest, OpenedDiff, PinnedBy};
pub use open::Core;

use crate::git::{GitError, ResolveError, SnapshotError};
use crate::ids::IdError;
use crate::objects::ObjectError;
use crate::paths::PathsError;
use crate::store::StoreError;

/// Errors from the review domain. Later tasks add variants (T1.13: cap, anchor,
/// ownership). [`CoreError::code`] gives the agent-facing code (design §15.1).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Paths(#[from] PathsError),
    /// Git failed, including `NotARepo` for a path outside any repository.
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Objects(#[from] ObjectError),
    /// A malformed diff id or prefix (fewer than 8 hex chars, not hex).
    #[error(transparent)]
    Id(#[from] IdError),
    /// No such review, diff or other record.
    #[error("{what} not found: {id}")]
    NotFound { what: &'static str, id: String },
    /// The diff is known but no known repo on disk has both of its trees.
    #[error("no repository on this machine has the trees of diff {0}")]
    RepoNotFound(String),
    /// A diff id prefix matches more than one stored diff.
    #[error("diff id prefix {prefix} is ambiguous: {}", matches.join(", "))]
    Ambiguous {
        prefix: String,
        matches: Vec<String>,
    },
    /// The request contradicts the stored state (e.g. pinning a live state into a
    /// review of another worktree, or into a commit review).
    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Store(StoreError::Sqlite(e))
    }
}

impl CoreError {
    /// The agent-facing error code (design §15.1 "Results"): `not_found`,
    /// `repo_not_found`, `objects_missing`, `conflict`, or `internal` for everything
    /// else. An ambiguous prefix is `not_found` (no single diff matches).
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::NotFound { .. } | CoreError::Ambiguous { .. } | CoreError::Id(_) => {
                "not_found"
            }
            CoreError::RepoNotFound(_) | CoreError::Git(GitError::NotARepo(_)) => "repo_not_found",
            CoreError::Resolve(ResolveError::ObjectsMissing(_))
            | CoreError::Objects(ObjectError::Missing(_)) => "objects_missing",
            CoreError::Resolve(ResolveError::Git(GitError::NotARepo(_))) => "repo_not_found",
            CoreError::Resolve(ResolveError::BadRevision(_)) => "not_found",
            CoreError::Conflict(_) => "conflict",
            _ => "internal",
        }
    }

    pub(crate) fn not_found(what: &'static str, id: impl Into<String>) -> CoreError {
        CoreError::NotFound {
            what,
            id: id.into(),
        }
    }
}
