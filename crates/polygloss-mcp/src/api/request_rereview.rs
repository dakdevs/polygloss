//! `request_rereview` (design §15.2, §17): the agent addressed the feedback.
//!
//! - A live review pins its worktree now as a new iteration
//!   (`pinned_by = rereview`; the latest one when nothing changed), on the base
//!   of a fresh open. Commit and compare reviews re-review their latest
//!   iteration (the app offers a moved ref as a new iteration itself).
//! - Sets `status = rereview_requested` and the summary, and appends
//!   `review.rereview_requested`.
//! - The human is notified: a running app learns it from its feed (nudged);
//!   one that is not running is launched in the background for it (not for a
//!   muted review).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::app_link;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `request_rereview` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestRereviewRequest {
    pub review_id: String,
    /// What changed since the last submission (markdown).
    pub summary_md: String,
}

/// `request_rereview` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestRereviewResult {
    pub review_id: String,
    /// Always `rereview_requested`.
    pub status: &'static str,
    /// The iteration to re-review (`seq`).
    pub iteration: u32,
    pub diff_id: String,
}

/// Pins the live state as a new iteration and asks the human to review again.
pub fn request_rereview(
    ctx: &ApiContext,
    req: RequestRereviewRequest,
) -> Result<RequestRereviewResult, ApiError> {
    if req.summary_md.trim().is_empty() {
        return Err(ApiError::conflict(
            "summary_md is empty: say what changed since the last review",
        ));
    }
    let actor = ctx.actor();
    let reopened = ctx.core.reopen_live(&req.review_id, &actor)?;
    let live = match &reopened {
        Some(opened) => Some((
            &opened.base,
            opened
                .live
                .as_ref()
                .ok_or_else(|| ApiError::internal("a live open carried no live state"))?,
        )),
        None => None,
    };
    let iteration = ctx
        .core
        .request_rereview(&req.review_id, &req.summary_md, &actor, live)?;
    app_link::after_write(ctx);
    app_link::launch_to_notify(ctx, &req.review_id);
    Ok(RequestRereviewResult {
        review_id: req.review_id,
        status: "rereview_requested",
        iteration: iteration.seq,
        diff_id: iteration.diff_id.as_str().to_owned(),
    })
}
