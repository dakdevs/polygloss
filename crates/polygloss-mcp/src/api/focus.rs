//! `focus` (design §15.2, §13.3): point the human at a location. The app
//! scrolls its review tab (opening it if needed) to a file, a line or a thread,
//! launching in the background when it is not running. Never opens an
//! external editor and never brings the app forward.
//!
//! The ids are checked here first (`not_found`); `line` is 1-based and needs
//! `path`. The app's own errors (a path not in the diff) keep their code.

use polygloss_core::ipc::protocol::Op;
use polygloss_core::review::Viewer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::shapes::SideParam;
use crate::app_link::{self, FocusStatus};
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `focus` params. Pass `diff_id` or `review_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FocusRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<SideParam>,
    /// 1-based line on `side`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// `focus` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FocusResult {
    pub status: FocusStatus,
}

/// Scrolls the Polygloss app to a location, launching it if needed.
pub fn focus(ctx: &ApiContext, req: FocusRequest) -> Result<FocusResult, ApiError> {
    let op = focus_op(ctx, req)?;
    Ok(FocusResult {
        status: app_link::focus(ctx, op)?,
    })
}

/// The checked socket op for `req`.
fn focus_op(ctx: &ApiContext, req: FocusRequest) -> Result<Op, ApiError> {
    let blank = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
    };
    if blank(&req.review_id) && blank(&req.diff_id) {
        return Err(ApiError::conflict("pass review_id or diff_id"));
    }
    let path = req.path.filter(|p| !p.trim().is_empty());
    if req.line == Some(0) {
        return Err(ApiError::conflict("line is 1-based"));
    }
    if req.line.is_some() && path.is_none() {
        return Err(ApiError::conflict("line needs path"));
    }
    let review_id = match req.review_id.filter(|r| !r.trim().is_empty()) {
        Some(r) => {
            // `NotFound` for an unknown review.
            ctx.core.iterations(&r)?;
            Some(r)
        }
        None => None,
    };
    let diff_id = match req.diff_id.filter(|d| !d.trim().is_empty()) {
        Some(d) => Some(ctx.core.resolve_diff_prefix(&d)?.as_str().to_owned()),
        None => None,
    };
    if let Some(t) = &req.thread_id {
        ctx.core.thread(t, Viewer::Agent)?;
    }
    Ok(Op::Focus {
        review_id,
        diff_id,
        path,
        side: req.side.map(Into::into),
        line: req.line,
        thread_id: req.thread_id,
    })
}
