//! `create_comment` (design §15.2, §8.1, §8.4): a published agent note or
//! question on a line, range, file or the whole review. Never a draft.
//!
//! Which diff the anchor refers to:
//!
//! - `review_id` of a live review: its worktree is snapshotted now and pinned
//!   on the base of that fresh open (`Core::pin_live_on_base`,
//!   `pinned_by = agent`); an unchanged worktree reuses the latest iteration.
//! - `review_id` of a commit or compare review: its latest iteration.
//! - `diff_id` (a unique prefix of 8+ hex digits works): that stored diff, in
//!   `review_id` when given (the diff must be one of its iterations), else in
//!   the most recent review showing it.
//!
//! The anchor is validated against that diff (`invalid_anchor`): lines are
//! 1-based lines of the file on `side` (default `new`), `start_line <= line`.
//! At most 50 agent threads per iteration (`cap_exceeded`).

use polygloss_core::DiffId;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{CoreError, IterationInfo, NewThread, PinnedBy, Subject, ThreadKind};
use polygloss_diff::Side;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::shapes::{AgentThreadKind, AnchorParam};
use crate::app_link;
use crate::context::ApiContext;
use crate::errors::{ApiError, ApiErrorCode};

/// `create_comment` params. Pass `review_id` or `diff_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateCommentRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_id: Option<String>,
    /// `note` explains a change; `question` asks the human for a decision.
    pub kind: AgentThreadKind,
    /// Markdown.
    pub body_md: String,
    /// Omit for a review-level comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorParam>,
}

/// `create_comment` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateCommentResult {
    pub thread_id: String,
    /// The diff the anchor refers to.
    pub diff_id: String,
    /// That diff's iteration `seq` in the review.
    pub iteration: u32,
}

/// Creates a published note or question thread.
pub fn create_comment(
    ctx: &ApiContext,
    req: CreateCommentRequest,
) -> Result<CreateCommentResult, ApiError> {
    let subject = subject(req.anchor.as_ref())?;
    let (review_id, iteration, blobs) =
        target(ctx, req.review_id.as_deref(), req.diff_id.as_deref())?;
    let thread_id = ctx.core.create_thread(
        &NewThread {
            review_id,
            diff_id: iteration.diff_id.clone(),
            subject,
            kind: match req.kind {
                AgentThreadKind::Note => ThreadKind::Note,
                AgentThreadKind::Question => ThreadKind::Question,
            },
            body_md: req.body_md,
            author: ctx.author(),
        },
        &blobs,
    )?;
    app_link::after_write(ctx);
    Ok(CreateCommentResult {
        thread_id,
        diff_id: iteration.diff_id.as_str().to_owned(),
        iteration: iteration.seq,
    })
}

/// The subject an anchor names: none = the review, `{path}` = the file,
/// `{path, line, …}` = lines (side `new` unless given).
pub fn subject(anchor: Option<&AnchorParam>) -> Result<Subject, ApiError> {
    let Some(a) = anchor else {
        return Ok(Subject::Review);
    };
    let invalid = |msg: &str| Err(ApiError::new(ApiErrorCode::InvalidAnchor, msg));
    if a.path.trim().is_empty() {
        return invalid("anchor.path is empty");
    }
    match a.line {
        None if a.side.is_some() || a.start_line.is_some() => invalid(
            "anchor.line is required with side or start_line (omit all three for a file comment)",
        ),
        None => Ok(Subject::File {
            path: a.path.clone(),
        }),
        Some(line) => Ok(Subject::Line {
            path: a.path.clone(),
            side: a.side.map_or(Side::New, Side::from),
            start_line: a.start_line.unwrap_or(line),
            line,
        }),
    }
}

/// The review, the iteration whose diff the comment anchors to, and a reader
/// for that diff's objects.
fn target(
    ctx: &ApiContext,
    review_id: Option<&str>,
    diff_id: Option<&str>,
) -> Result<(String, IterationInfo, BlobReader), ApiError> {
    let core = &ctx.core;
    match (review_id, diff_id) {
        (None, None) => Err(ApiError::conflict("pass review_id or diff_id")),
        (Some(review), None) => {
            let actor = ctx.actor();
            if let Some(opened) = core.reopen_live(review, &actor)? {
                let state = opened
                    .live
                    .as_ref()
                    .ok_or_else(|| ApiError::internal("a live open carried no live state"))?;
                let it =
                    core.pin_live_on_base(review, &opened.base, state, PinnedBy::Agent, &actor)?;
                let blobs = BlobReader::open(&opened.repo).map_err(CoreError::from)?;
                return Ok((review.to_owned(), it, blobs));
            }
            let it = core.iterations(review)?.pop().ok_or_else(|| {
                ApiError::conflict(format!("review {review} has no iteration yet"))
            })?;
            let blobs = blobs_for(ctx, &it.diff_id)?;
            Ok((review.to_owned(), it, blobs))
        }
        (review, Some(prefix)) => {
            let diff = core.resolve_diff_prefix(prefix)?;
            let review = match review {
                Some(r) => r.to_owned(),
                None => core
                    .latest_review_for_diff(diff.as_str())?
                    .ok_or_else(|| ApiError::not_found(format!("no review shows diff {diff}")))?,
            };
            let it = core
                .iterations(&review)?
                .into_iter()
                .rev()
                .find(|it| it.diff_id == diff)
                .ok_or_else(|| {
                    ApiError::conflict(format!(
                        "diff {diff} is not an iteration of review {review}"
                    ))
                })?;
            let blobs = blobs_for(ctx, &diff)?;
            Ok((review, it, blobs))
        }
    }
}

/// A reader for the objects of a repo that has both trees of `diff`.
fn blobs_for(ctx: &ApiContext, diff: &DiffId) -> Result<BlobReader, ApiError> {
    let (repo, _) = ctx.core.find_repo_for_diff(diff.as_str(), None)?;
    Ok(BlobReader::open(&repo).map_err(CoreError::from)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::shapes::SideParam;

    fn anchor(side: Option<SideParam>, line: Option<u32>, start: Option<u32>) -> AnchorParam {
        AnchorParam {
            path: "a.txt".into(),
            side,
            line,
            start_line: start,
        }
    }

    #[test]
    fn anchors_map_to_subjects() {
        assert_eq!(subject(None).unwrap(), Subject::Review);
        assert_eq!(
            subject(Some(&anchor(None, None, None))).unwrap(),
            Subject::File {
                path: "a.txt".into()
            }
        );
        assert_eq!(
            subject(Some(&anchor(None, Some(4), None))).unwrap(),
            Subject::Line {
                path: "a.txt".into(),
                side: Side::New,
                start_line: 4,
                line: 4
            }
        );
        assert_eq!(
            subject(Some(&anchor(Some(SideParam::Old), Some(4), Some(2)))).unwrap(),
            Subject::Line {
                path: "a.txt".into(),
                side: Side::Old,
                start_line: 2,
                line: 4
            }
        );
    }

    #[test]
    fn incomplete_anchors_are_invalid() {
        for a in [
            anchor(Some(SideParam::New), None, None),
            anchor(None, None, Some(3)),
            AnchorParam {
                path: " ".into(),
                side: None,
                line: None,
                start_line: None,
            },
        ] {
            let err = subject(Some(&a)).unwrap_err();
            assert_eq!(err.code, ApiErrorCode::InvalidAnchor, "{a:?}");
        }
    }
}
