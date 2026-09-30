//! The transport-agnostic agent API: one function per MCP tool (design §15.2),
//! shared by the rmcp server and the JSON CLI so their shapes cannot drift.
//!
//! Conventions every tool follows:
//!
//! - `pub fn <tool>(ctx: &ApiContext, req: <Tool>Request) -> Result<T, ApiError>`
//!   with `T: Serialize` (the §15.2 result object). The functions are blocking
//!   (core is sync); the MCP server runs them on tokio's blocking pool.
//! - Every write calls [`crate::nudge_app`] with the seq of its last event.
//! - Request types derive `JsonSchema`; their doc comments are the MCP input
//!   schema descriptions.

pub mod create_comment;
pub mod delete_comment;
pub mod edit_comment;
pub mod focus;
pub mod get_thread;
pub mod list_reviews;
pub mod list_threads;
pub mod open_diff;
pub mod reply;
pub mod request_rereview;
pub mod resolve;
pub mod resources;
pub mod shapes;
pub mod thread_summary;
pub mod wait_for_review;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub use create_comment::{CreateCommentRequest, CreateCommentResult, create_comment};
pub use delete_comment::{DeleteCommentRequest, DeleteCommentResult, delete_comment};
pub use edit_comment::{EditCommentRequest, EditCommentResult, edit_comment};
pub use focus::{FocusRequest, FocusResult, focus};
pub use get_thread::{GetThreadRequest, GetThreadResult, get_thread};
pub use list_reviews::{ListReviewsRequest, ListReviewsResult, list_reviews};
pub use list_threads::{ListThreadsRequest, ListThreadsResult, list_threads};
pub use open_diff::{OpenDiffRequest, OpenDiffResult, open_diff};
pub use reply::{ReplyRequest, ReplyResult, reply};
pub use request_rereview::{RequestRereviewRequest, RequestRereviewResult, request_rereview};
pub use resolve::{
    ResolveRequest, ResolveResult, UnresolveRequest, UnresolveResult, resolve, unresolve,
};
pub use wait_for_review::{WaitForReviewRequest, wait_for_review};

/// A progress callback: `(elapsed, total)` since the wait started.
pub type ProgressFn = Box<dyn Fn(Duration, Duration) + Send + Sync>;

/// How a blocking call (`wait_for_review`) learns it was cancelled and reports
/// progress. The MCP server wires `cancelled` to the request's cancellation token
/// and `progress` to `notifications/progress` when the call has a
/// `progressToken`; the JSON CLI passes [`WaitControl::default`].
#[derive(Default)]
pub struct WaitControl {
    cancelled: Arc<AtomicBool>,
    progress: Option<ProgressFn>,
}

impl WaitControl {
    /// A control with a progress callback.
    pub fn with_progress(progress: ProgressFn) -> WaitControl {
        WaitControl {
            cancelled: Arc::default(),
            progress: Some(progress),
        }
    }

    /// The flag [`WaitControl::is_cancelled`] reads; set it to cancel.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Whether anyone listens to progress.
    pub fn wants_progress(&self) -> bool {
        self.progress.is_some()
    }

    /// Reports progress (no-op without a listener).
    pub fn progress(&self, elapsed: Duration, total: Duration) {
        if let Some(p) = &self.progress {
            p(elapsed, total);
        }
    }
}

impl std::fmt::Debug for WaitControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitControl")
            .field("cancelled", &self.is_cancelled())
            .field("progress", &self.progress.is_some())
            .finish()
    }
}
