//! `resolve` and `unresolve` (design §15.2, §8.2): immediate (OQ-10), recording
//! who and when (`thread.resolved` / `thread.unresolved`). Setting the current
//! status again changes nothing (a thread someone else resolved keeps their
//! `resolved_by`). `resolve` may add a closing reply first, in the same
//! transaction. Draft threads do not exist for agents (`not_found`).

use polygloss_core::review::{AuthorKind, ThreadStatus, Viewer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::shapes::timestamp;
use crate::app_link;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `resolve` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolveRequest {
    pub thread_id: String,
    /// Optional closing reply (markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_md: Option<String>,
}

/// `unresolve` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnresolveRequest {
    pub thread_id: String,
}

/// Who resolved a thread and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedByOut {
    pub kind: AuthorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub at: String,
}

/// `resolve` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolveResult {
    pub thread_id: String,
    pub status: ThreadStatus,
    pub resolved_by: ResolvedByOut,
}

/// `unresolve` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnresolveResult {
    pub thread_id: String,
    pub status: ThreadStatus,
}

/// Resolves a thread, optionally with a closing reply.
pub fn resolve(ctx: &ApiContext, req: ResolveRequest) -> Result<ResolveResult, ApiError> {
    ctx.core
        .set_resolved(&req.thread_id, true, &ctx.actor(), req.body_md.as_deref())?;
    app_link::after_write(ctx);
    let thread = ctx.core.thread(&req.thread_id, Viewer::Agent)?;
    let by = thread
        .resolved_by
        .ok_or_else(|| ApiError::conflict(format!("thread {} was reopened", req.thread_id)))?;
    Ok(ResolveResult {
        thread_id: req.thread_id,
        status: thread.status,
        resolved_by: ResolvedByOut {
            kind: by.kind,
            name: by.name,
            at: timestamp(by.at),
        },
    })
}

/// Reopens a resolved thread.
pub fn unresolve(ctx: &ApiContext, req: UnresolveRequest) -> Result<UnresolveResult, ApiError> {
    ctx.core
        .set_resolved(&req.thread_id, false, &ctx.actor(), None)?;
    app_link::after_write(ctx);
    Ok(UnresolveResult {
        thread_id: req.thread_id,
        status: ThreadStatus::Open,
    })
}
