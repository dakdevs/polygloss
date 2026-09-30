//! `list_reviews` (design §15.2): reviews with status, viewed counts, open
//! threads and questions and the last submission, most recently active first.
//!
//! - `repo` narrows to the repository containing that path (`repo_not_found`
//!   outside any); omitted, every repo is listed (no repo default here).
//! - `assigned = "me"` keeps reviews assigned to this session or any id linked
//!   to its canonical session.
//! - Archived reviews are not listed. Pages hold `limit` reviews (default 50,
//!   at most [`MAX_LIMIT`]) and stay under [`PAGE_BUDGET`] characters; `next_cursor`
//!   continues after the last one (keyset on `updated_at`, `review_id`).

use polygloss_core::git::{self, ReviewKind};
use polygloss_core::review::{CoreError, ReviewFilter, ReviewSummary, SummaryCursor, Verdict};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::shapes::{AssignedFilter, BODY_MAX_CHARS, PAGE_BUDGET, timestamp, truncate_chars};
use crate::context::ApiContext;
use crate::errors::ApiError;
use crate::paging::{Cursor, decode_cursor, encode_cursor, fit_page};

/// The page size when `limit` is omitted.
pub const DEFAULT_LIMIT: u32 = 50;
/// The largest `limit` honored.
pub const MAX_LIMIT: u32 = 200;
/// The statuses a review can have (`reviews.status`).
pub const STATUSES: [&str; 5] = [
    "open",
    "changes_requested",
    "commented",
    "approved",
    "rereview_requested",
];

/// `list_reviews` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListReviewsRequest {
    /// Only reviews of the repository containing this path (omit for all repos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Only reviews with this status: `open`, `changes_requested`, `commented`,
    /// `approved` or `rereview_requested`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `any` (default) or `me`: only reviews assigned to this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned: Option<AssignedFilter>,
    /// `next_cursor` from the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size (default 50, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `list_reviews` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListReviewsResult {
    pub reviews: Vec<ReviewOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One review of `list_reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewOut {
    pub review_id: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub kind: ReviewKind,
    /// The repository's main worktree (or the git dir of a bare repo).
    pub repo: String,
    pub status: String,
    pub iterations: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_diff_id: Option<String>,
    pub viewed: Viewed,
    pub open_threads: u32,
    pub open_questions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_submission: Option<LastSubmissionOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rereview: Option<RereviewOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_session: Option<String>,
    pub updated_at: String,
}

/// Files of the latest iteration marked Viewed, of all its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Viewed {
    pub done: u32,
    pub total: u32,
}

/// The newest submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastSubmissionOut {
    pub verdict: Verdict,
    pub summary_md: String,
    /// The summary was longer than 20k characters and was cut.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub summary_truncated: bool,
    pub at: String,
}

/// A pending re-review request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RereviewOut {
    pub summary: String,
    pub at: String,
}

impl From<ReviewSummary> for ReviewOut {
    fn from(s: ReviewSummary) -> ReviewOut {
        ReviewOut {
            review_id: s.review_id,
            key: s.key,
            label: s.label,
            kind: s.kind,
            repo: s.repo_path.to_string_lossy().into_owned(),
            status: s.status,
            iterations: s.iterations,
            latest_diff_id: s.latest_diff_id.map(String::from),
            viewed: Viewed {
                done: s.viewed_done,
                total: s.viewed_total,
            },
            open_threads: s.open_threads,
            open_questions: s.open_questions,
            last_submission: s.last_submission.map(|sub| {
                let (summary_md, summary_truncated) =
                    truncate_chars(&sub.summary_md, BODY_MAX_CHARS);
                LastSubmissionOut {
                    verdict: sub.verdict,
                    summary_md,
                    summary_truncated,
                    at: timestamp(sub.at),
                }
            }),
            rereview: s.rereview.map(|(summary, at)| RereviewOut {
                summary: truncate_chars(&summary, BODY_MAX_CHARS).0,
                at: timestamp(at),
            }),
            assigned_session: s.assigned_session,
            updated_at: timestamp(s.updated_at),
        }
    }
}

/// Lists reviews, most recently active first.
pub fn list_reviews(
    ctx: &ApiContext,
    req: ListReviewsRequest,
) -> Result<ListReviewsResult, ApiError> {
    let repo_common_dir = match req.repo.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(repo) => {
            let path = ctx.default_repo(Some(repo));
            Some(
                git::discover(&path)
                    .map_err(|e| ApiError::from(CoreError::from(e)))?
                    .common_dir,
            )
        }
        None => None,
    };
    let status = req
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(s) = status
        && !STATUSES.contains(&s)
    {
        return Err(ApiError::conflict(format!(
            "unknown status {s:?}; use one of {}",
            STATUSES.join(", ")
        )));
    }
    let me = req.assigned == Some(AssignedFilter::Me);
    let query = json!({
        "repo": repo_common_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "status": status,
        "assigned": me.then_some(ctx.session_id.as_str()),
    })
    .to_string();
    let after = match req.cursor.as_deref().filter(|c| !c.trim().is_empty()) {
        Some(c) => {
            let cursor = decode_cursor(c)?;
            cursor.check_query("reviews", &query)?;
            Some(cursor.after_as::<SummaryCursor>()?)
        }
        None => None,
    };
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut rows = ctx.core.review_summaries(&ReviewFilter {
        repo_common_dir,
        status: status.map(str::to_owned),
        assigned_session: me.then(|| ctx.session_id.clone()),
        after,
        limit: Some(limit + 1),
        ..ReviewFilter::default()
    })?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let cursors: Vec<SummaryCursor> = rows.iter().map(ReviewSummary::cursor).collect();
    let reviews: Vec<ReviewOut> = rows.into_iter().map(ReviewOut::from).collect();
    let (reviews, cut) = fit_page(reviews, PAGE_BUDGET);
    let next_cursor = if cut || more {
        let last = &cursors[reviews.len() - 1];
        Some(encode_cursor(&Cursor::new("reviews", query, last)?))
    } else {
        None
    };
    Ok(ListReviewsResult {
        reviews,
        next_cursor,
    })
}
