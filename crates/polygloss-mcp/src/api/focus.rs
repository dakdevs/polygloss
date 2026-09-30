//! `focus` (design §15.2, §13.3): point the human at a location. The app
//! scrolls its review tab (opening it if needed) to a file, a line or a thread,
//! launching in the background when it is not running. Never opens an
//! external editor and never brings the app forward.
//!
//! The ids are checked here first (`not_found`); `line` is 1-based and needs
//! `path`. The location is checked against the diff positions refer to
//! (`diff_id`, else the review's latest iteration, design §15.2): a path not in
//! it, or a side the file does not have (the old side of an added file), is
//! `not_found`; a line past the end of that side is `conflict`. The app checks
//! again against the diff its tab shows, and its errors keep their code.

use polygloss_core::ipc::protocol::Op;
use polygloss_core::review::Viewer;
use polygloss_diff::lines::LineIndex;
use polygloss_diff::{FileKind, FileStatus, Side};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::shapes::SideParam;
use crate::api::thread_summary::{DiffContext, latest_diff};
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

/// Checks `req` as [`focus`] does (`conflict`, `not_found`) without talking
/// to the app: the JSON CLI's `--no-open focus`, which then reports
/// `unavailable`.
pub fn check(ctx: &ApiContext, req: FocusRequest) -> Result<(), ApiError> {
    focus_op(ctx, req).map(|_| ())
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
    let side = req.side.map(Side::from);
    if let Some(path) = &path {
        let target = match &diff_id {
            Some(d) => Some(polygloss_core::DiffId::parse(d).map_err(|e| {
                ApiError::internal(format!("resolved diff id {d} is invalid: {e}"))
            })?),
            None => latest_diff(ctx, review_id.as_deref())?,
        };
        if let Some(target) = target {
            check_location(ctx, &target, path, side.unwrap_or(Side::New), req.line)?;
        }
    }
    Ok(Op::Focus {
        review_id,
        diff_id,
        path,
        side,
        line: req.line,
        thread_id: req.thread_id,
    })
}

/// `path` (and `line` on `side`) exist in diff `diff_id`: `not_found` for a
/// path not in the diff or a side the file lacks, `conflict` for a line past
/// the end. Unreadable objects (the repo is gone) skip the line check.
fn check_location(
    ctx: &ApiContext,
    diff_id: &polygloss_core::DiffId,
    path: &str,
    side: Side,
    line: Option<u32>,
) -> Result<(), ApiError> {
    let dc = DiffContext::load(ctx, diff_id)?;
    let file = dc
        .files
        .iter()
        .find(|f| {
            f.display_path() == path
                || (side == Side::Old && f.old_path.as_ref().is_some_and(|p| p.text == path))
        })
        .ok_or_else(|| ApiError::not_found(format!("{path} is not in diff {diff_id}")))?;
    let (absent, what, other) = match side {
        Side::Old => (file.status == FileStatus::Added, "added", "new"),
        Side::New => (file.status == FileStatus::Deleted, "deleted", "old"),
    };
    let name = if side == Side::Old { "old" } else { "new" };
    if absent {
        return Err(ApiError::not_found(format!(
            "{path} is {what} in diff {diff_id}: it has no {name} side (use side={other})"
        )));
    }
    let Some(line) = line else {
        return Ok(());
    };
    if matches!(file.kind, FileKind::Binary | FileKind::Submodule) {
        return Err(ApiError::conflict(format!(
            "{path} is not a text file: focus it without a line"
        )));
    }
    let blob = match side {
        Side::Old => &file.old_blob,
        Side::New => &file.new_blob,
    };
    let Some(bytes) = dc.blobs.as_ref().and_then(|b| b.read(blob).ok()) else {
        return Ok(());
    };
    let n = LineIndex::new(&bytes).len();
    if line > n {
        return Err(ApiError::conflict(format!(
            "{path} has {n} lines on the {name} side; line {line} is past its end"
        )));
    }
    Ok(())
}
