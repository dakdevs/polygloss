//! `wait_for_review` (design §15.2): blocks until the human submits or
//! archives a review, for clients without the Polygloss plugin's Stop hook
//! (§16.4 "Other clients").
//!
//! Rules:
//!
//! - It watches the review's `review.submitted` and `review.archived` events
//!   after `since` (default: the latest event seq when the call starts; a
//!   `since` above it is clamped to it) with an
//!   [`EventFeed`] polled every [`POLL`] (a `PRAGMA data_version` read while
//!   nothing changes). Anything already there returns at once.
//! - A submission wins over an archive in the same batch (an approval followed
//!   by archiving still reports the verdict; the next call reports the
//!   archive). Of several submissions the latest is reported. A review that is
//!   archived when the call starts, or pruned (`review.archived` with
//!   `pruned`), returns `archived`; an unknown review is `not_found`.
//! - `threads` are the review's threads with activity after `since`, exactly as
//!   `list_threads(review_id, since, status=all)` summarizes them (first
//!   [`list_threads::MAX_LIMIT`]), cut to keep the result within the page budget
//!   (`threads_truncated: true`; `list_threads` pages through the rest).
//! - `next_since` is the feed's cursor: every submission or archive of the
//!   review up to it was considered, so passing it back never misses one.
//! - `timeout_s` defaults to and is capped at [`MAX_TIMEOUT_S`] (1,500 s, under
//!   Claude Code's 30-minute stdio idle window; **Provisional**). When the call
//!   has a `progressToken` a progress notification goes out every
//!   [`PROGRESS_EVERY`] (`POLYGLOSS_WAIT_PROGRESS_MS` overrides it in test mode,
//!   OQ-P4). Cancellation (the request's `ct`, or the server shutting down)
//!   ends the wait within one poll.

use std::time::{Duration, Instant};

use polygloss_core::review::{CoreError, Verdict};
use polygloss_core::store::events::{Event, EventFeed, EventFilter, EventKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::WaitControl;
use crate::api::list_threads::{self, ListThreadsRequest, Query};
use crate::api::shapes::{
    BODY_MAX_CHARS, PAGE_BUDGET, ThreadStatusFilter, ThreadSummary, timestamp, truncate_chars,
};
use crate::context::ApiContext;
use crate::errors::ApiError;
use crate::paging::fit_page;

/// `timeout_s` when omitted (design §15.2, **Provisional**).
pub const DEFAULT_TIMEOUT_S: u32 = 1500;
/// The largest `timeout_s` honored; larger values are clamped.
pub const MAX_TIMEOUT_S: u32 = 1500;
/// How often the event feed is polled.
pub const POLL: Duration = Duration::from_millis(250);
/// How often progress is reported to a caller that sent a `progressToken`.
pub const PROGRESS_EVERY: Duration = Duration::from_secs(60);
/// Test-only override of [`PROGRESS_EVERY`] in milliseconds (with
/// `POLYGLOSS_TEST=1`, OQ-P4).
pub const PROGRESS_ENV: &str = "POLYGLOSS_WAIT_PROGRESS_MS";

/// `wait_for_review` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WaitForReviewRequest {
    pub review_id: String,
    /// Event seq to wait after (`next_since` of the previous call). Default: now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Seconds to wait (default and max 1500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u32>,
}

/// The submission a wait reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmissionOut {
    pub submission_id: String,
    /// `request_changes`, `comment` or `approve` (`approve` means done).
    pub verdict: Verdict,
    pub summary_md: String,
    /// The summary was longer than 20k characters and was cut.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub summary_truncated: bool,
    /// The iteration (`seq`) the human submitted against.
    pub iteration: u32,
    pub at: String,
}

/// `wait_for_review` result: `{outcome: "submitted", submission, threads,
/// next_since}` or `{outcome: "timeout" | "archived", next_since}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WaitForReviewResult {
    Submitted {
        submission: SubmissionOut,
        /// Threads with activity after `since` (new or updated).
        threads: Vec<ThreadSummary>,
        /// More threads changed than fit; call `list_threads(since=…)`.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        threads_truncated: bool,
        next_since: i64,
    },
    Timeout {
        next_since: i64,
    },
    Archived {
        next_since: i64,
    },
}

/// How often a wait polls and reports progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitTiming {
    pub poll: Duration,
    pub progress_every: Duration,
}

impl Default for WaitTiming {
    fn default() -> Self {
        WaitTiming {
            poll: POLL,
            progress_every: PROGRESS_EVERY,
        }
    }
}

impl WaitTiming {
    /// The default timing, with [`PROGRESS_ENV`] (a positive number of
    /// milliseconds) honored only when `POLYGLOSS_TEST=1`.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> WaitTiming {
        let mut timing = WaitTiming::default();
        let test_mode = env(polygloss_core::ipc::TEST_ENV).is_some_and(|v| v.trim() == "1");
        if test_mode
            && let Some(ms) = env(PROGRESS_ENV).and_then(|v| v.trim().parse::<u64>().ok())
            && ms > 0
        {
            timing.progress_every = Duration::from_millis(ms);
        }
        timing
    }
}

/// The wait's length: `timeout_s` (default [`DEFAULT_TIMEOUT_S`]) capped at
/// [`MAX_TIMEOUT_S`].
pub fn effective_timeout(timeout_s: Option<u32>) -> Duration {
    Duration::from_secs(u64::from(
        timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).min(MAX_TIMEOUT_S),
    ))
}

/// Blocks until the human submits or archives the review, or the timeout.
pub fn wait_for_review(
    ctx: &ApiContext,
    req: WaitForReviewRequest,
    ctl: &WaitControl,
) -> Result<WaitForReviewResult, ApiError> {
    let timing = WaitTiming::from_env(|k| std::env::var(k).ok());
    wait_for_review_with(ctx, req, ctl, &timing)
}

/// [`wait_for_review`] with explicit timing (tests).
pub fn wait_for_review_with(
    ctx: &ApiContext,
    req: WaitForReviewRequest,
    ctl: &WaitControl,
    timing: &WaitTiming,
) -> Result<WaitForReviewResult, ApiError> {
    let review_id = req.review_id.trim().to_owned();
    if review_id.is_empty() {
        return Err(ApiError::conflict("pass review_id"));
    }
    let timeout = effective_timeout(req.timeout_s);
    let filter = EventFilter {
        review_ids: Some(vec![review_id.clone()]),
        agent_visible_only: false,
        kinds: Some(vec![EventKind::ReviewSubmitted, EventKind::ReviewArchived]),
    };
    let mut feed = EventFeed::open(&ctx.core.paths, req.since.unwrap_or(0).max(0), filter)
        .map_err(store_error)?;
    // The feed clamps a `since` above the latest seq (T5.9): a future cursor
    // would skip every event until the seq caught up.
    let since = match req.since {
        Some(_) => feed.cursor(),
        None => feed.seek_to_latest().map_err(store_error)?,
    };

    let started = Instant::now();
    let mut next_progress = timing.progress_every;
    let mut first = true;
    loop {
        let events = feed.poll().map_err(store_error)?;
        if let Some(r) = outcome(ctx, &review_id, since, &events, feed.cursor())? {
            return Ok(r);
        }
        if first {
            first = false;
            // An archived review would never be submitted; an unknown one is
            // `not_found` (a pruned one after `since` was reported above).
            if ctx.core.review_archived(&review_id)? {
                return Ok(WaitForReviewResult::Archived {
                    next_since: feed.cursor(),
                });
            }
        }
        if ctl.is_cancelled() {
            tracing::debug!(review_id = %review_id, "wait_for_review cancelled");
            return Err(ApiError::internal("wait_for_review cancelled"));
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Ok(WaitForReviewResult::Timeout {
                next_since: feed.cursor(),
            });
        }
        let mut nap = timing.poll.min(timeout - elapsed);
        if ctl.wants_progress() {
            if elapsed >= next_progress {
                ctl.progress(elapsed, timeout);
                while next_progress <= elapsed {
                    next_progress += timing.progress_every;
                }
            }
            nap = nap.min(next_progress - elapsed);
        }
        std::thread::sleep(nap);
    }
}

/// The result `events` (the review's submissions and archives after `since`,
/// in `seq` order) call for, if any.
fn outcome(
    ctx: &ApiContext,
    review_id: &str,
    since: i64,
    events: &[Event],
    next_since: i64,
) -> Result<Option<WaitForReviewResult>, ApiError> {
    // The latest submission that still exists (a pruned review's are gone).
    for e in events
        .iter()
        .rev()
        .filter(|e| e.kind == EventKind::ReviewSubmitted)
    {
        let Some(id) = e.payload["submission_id"].as_str() else {
            continue;
        };
        let Some(sub) = ctx.core.submission(id)? else {
            continue;
        };
        let (summary_md, summary_truncated) = truncate_chars(&sub.summary_md, BODY_MAX_CHARS);
        let submission = SubmissionOut {
            submission_id: sub.id,
            verdict: sub.verdict,
            summary_md,
            summary_truncated,
            iteration: sub.iteration.seq,
            at: timestamp(sub.submitted_at),
        };
        let (threads, threads_truncated) = changed_threads(ctx, review_id, since, &submission)?;
        return Ok(Some(WaitForReviewResult::Submitted {
            submission,
            threads,
            threads_truncated,
            next_since,
        }));
    }
    if events.iter().any(|e| e.kind == EventKind::ReviewArchived) {
        return Ok(Some(WaitForReviewResult::Archived { next_since }));
    }
    Ok(None)
}

/// The review's threads with activity after `since`, as `list_threads` shows
/// them, within what the page budget leaves next to `submission`.
fn changed_threads(
    ctx: &ApiContext,
    review_id: &str,
    since: i64,
    submission: &SubmissionOut,
) -> Result<(Vec<ThreadSummary>, bool), ApiError> {
    let query = Query::resolve(
        ctx,
        &ListThreadsRequest {
            review_id: Some(review_id.to_owned()),
            status: Some(ThreadStatusFilter::All),
            since: Some(since),
            ..ListThreadsRequest::default()
        },
    )?;
    let all = query.threads(ctx)?;
    let page: Vec<_> = all.iter().take(list_threads::MAX_LIMIT as usize).collect();
    let more = all.len() > page.len();
    let summaries = query.summaries(ctx, &page)?;
    let used = serde_json::to_string(submission)
        .map(|s| s.chars().count())
        .unwrap_or(0);
    let (threads, cut) = fit_page(summaries, PAGE_BUDGET.saturating_sub(used));
    Ok((threads, cut || more))
}

fn store_error(e: polygloss_core::store::StoreError) -> ApiError {
    ApiError::from(CoreError::from(e))
}
