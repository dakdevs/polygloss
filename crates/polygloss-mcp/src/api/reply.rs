//! `reply` (design §15.2): an agent reply, published at once
//! (`comment.created`), optionally resolving the thread in the same
//! transaction (`thread.resolved`; if resolving fails, the reply is not
//! published either). Replies to resolved or outdated threads are allowed and
//! do not reopen them. Draft threads do not exist for agents (`not_found`).

use polygloss_core::review::{ThreadStatus, Viewer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::app_link;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `reply` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplyRequest {
    pub thread_id: String,
    /// Markdown.
    pub body_md: String,
    /// Also resolve the thread (default false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve: Option<bool>,
}

/// `reply` result: the new comment and the thread's status after the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyResult {
    pub comment_id: String,
    pub thread_id: String,
    pub status: ThreadStatus,
}

/// Publishes a reply at once; with `resolve`, the reply and the resolve are
/// one transaction (both or neither).
pub fn reply(ctx: &ApiContext, req: ReplyRequest) -> Result<ReplyResult, ApiError> {
    let (comment_id, status) = if req.resolve.unwrap_or(false) {
        let id = ctx
            .core
            .reply_and_resolve(&req.thread_id, &req.body_md, &ctx.actor())?;
        (id, ThreadStatus::Resolved)
    } else {
        let id = ctx
            .core
            .reply(&req.thread_id, &req.body_md, &ctx.author())?;
        let status = ctx
            .core
            .thread(&req.thread_id, Viewer::Agent)
            .map(|t| t.status);
        // The reply is published either way: nudge before reporting a failure.
        app_link::after_write(ctx);
        return Ok(ReplyResult {
            comment_id: id,
            thread_id: req.thread_id,
            status: status?,
        });
    };
    app_link::after_write(ctx);
    Ok(ReplyResult {
        comment_id,
        thread_id: req.thread_id,
        status,
    })
}
