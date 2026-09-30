//! The review domain: reviews, iterations, threads, drafts, submissions, Viewed,
//! view state, sessions and carry-forward (T1.12–T1.15). Converts the 0-based lines
//! of `polygloss-diff` to the 1-based anchors of the store and agent surfaces.

pub mod activity;
pub mod carry_forward;
pub mod iterations;
pub mod lookup;
pub mod models;
pub mod open;
pub mod sessions;
pub mod submit;
pub mod suggestions;
pub mod summary;
pub mod threads;
pub mod view_state;
pub mod viewed;

pub use activity::{Rereview, ReviewActivity, UnreadThread};
pub use carry_forward::{CARRY_FORWARD_ENGINE_VERSION, Position, PositionState, position_of};
pub use iterations::{IterationEntry, LastSubmission};
pub use models::{IterationInfo, OpenRequest, OpenedDiff, PinnedBy};
pub use open::Core;
pub use sessions::{AssignedBy, ReplacedWaiter, SessionInfo};
pub use submit::{Submission, SubmitDraft, Verdict};
pub use suggestions::parse_suggestions;
pub use summary::{RecentRepo, ReviewFilter, ReviewSummary, SubmissionSummary, SummaryCursor};
pub use threads::{
    AGENT_THREAD_CAP, Author, AuthorKind, CommentView, DeletedComment, NewThread, ResolvedBy,
    Subject, ThreadAnchor, ThreadFilter, ThreadKind, ThreadScope, ThreadStatus, ThreadView, Viewer,
};
pub use view_state::{ScrollAnchorState, VIEW_STATE_VERSION, ViewState};
pub use viewed::ViewedState;

use crate::git::{GitError, ResolveError, SnapshotError};
use crate::ids::IdError;
use crate::objects::ObjectError;
use crate::paths::PathsError;
use crate::store::StoreError;

/// Errors from the review domain. Later tasks may add variants. [`CoreError::code`] gives the agent-facing code (design §15.1).
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
    /// An agent already created [`AGENT_THREAD_CAP`] threads in this iteration
    /// (design §8.4, OQ-13).
    #[error("agents may create at most {cap} threads per iteration")]
    CapExceeded { cap: u32 },
    /// The anchor does not fit the diff: path not in it, lines outside the blob,
    /// `start_line > line`, the old side of an added file, the new side of a
    /// deleted file, or lines of a binary or submodule entry (design §8.1).
    #[error("invalid anchor: {0}")]
    InvalidAnchor(String),
    /// Editing or deleting someone else's comment (design §8.2, OQ-30).
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// A request the domain rules reject whatever the stored state: an empty body,
    /// a note or question from a human, a resolve by the system actor.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Store(StoreError::Sqlite(e))
    }
}

impl CoreError {
    /// The agent-facing error code (design §15.1 "Results"): `not_found`,
    /// `repo_not_found`, `objects_missing`, `invalid_anchor`, `cap_exceeded`,
    /// `forbidden`, `conflict` (also for [`CoreError::InvalidRequest`]), or
    /// `internal` for everything else. An ambiguous prefix is `not_found` (no
    /// single diff matches).
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
            // §15.1 has no dedicated code for malformed requests; `conflict` tells the
            // caller to change the request rather than retry (`internal`).
            CoreError::Conflict(_) | CoreError::InvalidRequest(_) => "conflict",
            CoreError::CapExceeded { .. } => "cap_exceeded",
            CoreError::InvalidAnchor(_) => "invalid_anchor",
            CoreError::Forbidden(_) => "forbidden",
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
