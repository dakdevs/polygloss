//! `list_threads` (design §15.2): the threads of a review or diff that agents
//! may see (never drafts, §15.1 "Visibility"), oldest first, with positions.
//!
//! - `review_id`: the review's threads plus threads opened on its latest diff
//!   elsewhere (§8.6); `diff_id` alone: threads on that diff and of every review
//!   with an iteration on it. Positions are relative to `diff_id` when given,
//!   else to the review's latest iteration.
//! - `latest_seq` is read before the list, so `since = latest_seq` on the next
//!   call never misses what happened in between.
//! - Pages hold `limit` threads (default 50, at most [`MAX_LIMIT`]) and stay
//!   under [`PAGE_BUDGET`] characters; `next_cursor` continues after the last thread.

use polygloss_core::DiffId;
use polygloss_core::review::{
    AuthorKind, ThreadFilter, ThreadKind, ThreadScope, ThreadStatus, ThreadView, Viewer,
};
use polygloss_core::store::events::latest_seq;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::shapes::{
    AuthorFilter, PAGE_BUDGET, ThreadKindParam, ThreadStatusFilter, ThreadSummary,
};
use crate::api::thread_summary::{latest_diff, positions, summarize};
use crate::context::ApiContext;
use crate::errors::ApiError;
use crate::paging::{Cursor, decode_cursor, encode_cursor, fit_page};

/// The page size when `limit` is omitted.
pub const DEFAULT_LIMIT: u32 = 50;
/// The largest `limit` honored.
pub const MAX_LIMIT: u32 = 200;

/// `list_threads` params. Pass `review_id` or `diff_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListThreadsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    /// A diff id or an unambiguous prefix of at least 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_id: Option<String>,
    /// `open` (default), `resolved` or `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ThreadStatusFilter>,
    /// Who started the thread: `human`, `agent` or `any` (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<AuthorFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ThreadKindParam>,
    /// Only threads on this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Only threads with activity after this event seq (`latest_seq` / `next_since`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// `next_cursor` from the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size (default 50, at most 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `list_threads` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListThreadsResult {
    pub threads: Vec<ThreadSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The newest event seq when the list was read: pass it as `since` later.
    pub latest_seq: i64,
}

/// Where a page continues: after this thread (or, if it is gone, after
/// threads created before `created_at`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct After {
    created_at: i64,
    thread_id: String,
}

/// The scope, target diff and filters a request names.
pub(crate) struct Query {
    pub scope: ThreadScope,
    /// The diff positions are relative to (`None`: each thread's origin diff).
    pub target: Option<DiffId>,
    pub filter: ThreadFilter,
}

impl Query {
    /// Resolves the scope and filters of `req` (`conflict` without a review or
    /// diff; `not_found` for an unknown review or diff).
    pub(crate) fn resolve(ctx: &ApiContext, req: &ListThreadsRequest) -> Result<Query, ApiError> {
        let review = req
            .review_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let diff = match req
            .diff_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(d) => Some(ctx.core.resolve_diff_prefix(d)?),
            None => None,
        };
        let (scope, target) = match (review, diff) {
            (Some(r), diff) => {
                // Both paths are `not_found` for an unknown review (core checks).
                let target = match diff {
                    Some(d) => Some(d),
                    None => latest_diff(ctx, Some(r))?,
                };
                (ThreadScope::Review(r.to_owned()), target)
            }
            (None, Some(d)) => (ThreadScope::Diff(d.clone()), Some(d)),
            (None, None) => return Err(ApiError::conflict("pass review_id or diff_id")),
        };
        let filter = ThreadFilter {
            status: match req.status.unwrap_or_default() {
                ThreadStatusFilter::Open => Some(ThreadStatus::Open),
                ThreadStatusFilter::Resolved => Some(ThreadStatus::Resolved),
                ThreadStatusFilter::All => None,
            },
            author: match req.author.unwrap_or_default() {
                AuthorFilter::Human => Some(AuthorKind::Human),
                AuthorFilter::Agent => Some(AuthorKind::Agent),
                AuthorFilter::Any => None,
            },
            kind: req.kind.map(|k| match k {
                ThreadKindParam::Comment => ThreadKind::Comment,
                ThreadKindParam::Note => ThreadKind::Note,
                ThreadKindParam::Question => ThreadKind::Question,
            }),
            path: req.path.clone().filter(|p| !p.is_empty()),
            since_seq: req.since,
        };
        Ok(Query {
            scope,
            target,
            filter,
        })
    }

    /// The cursor fingerprint: scope, target and filters.
    fn fingerprint(&self) -> String {
        let scope = match &self.scope {
            ThreadScope::Review(r) => json!({ "review": r }),
            ThreadScope::Diff(d) => json!({ "diff": d.as_str() }),
        };
        json!({
            "scope": scope,
            "target": self.target.as_ref().map(DiffId::as_str),
            "status": self.filter.status.map(|s| s.as_str()),
            "author": self.filter.author.map(|a| a.as_str()),
            "kind": self.filter.kind.map(|k| k.as_str()),
            "path": self.filter.path,
            "since": self.filter.since_seq,
        })
        .to_string()
    }

    /// Every matching thread, oldest first.
    pub(crate) fn threads(&self, ctx: &ApiContext) -> Result<Vec<ThreadView>, ApiError> {
        Ok(ctx
            .core
            .threads(self.scope.clone(), Viewer::Agent, &self.filter)?)
    }

    /// Summaries of `threads` placed in the target diff.
    pub(crate) fn summaries(
        &self,
        ctx: &ApiContext,
        threads: &[&ThreadView],
    ) -> Result<Vec<ThreadSummary>, ApiError> {
        let (pos, _) = positions(ctx, threads, self.target.as_ref())?;
        Ok(threads
            .iter()
            .map(|t| summarize(t, pos.get(&t.id)))
            .collect())
    }
}

/// Lists the threads of a review or diff that agents may see.
pub fn list_threads(
    ctx: &ApiContext,
    req: ListThreadsRequest,
) -> Result<ListThreadsResult, ApiError> {
    let query = Query::resolve(ctx, &req)?;
    let fingerprint = query.fingerprint();
    let after = match req.cursor.as_deref().filter(|c| !c.trim().is_empty()) {
        Some(c) => {
            let cursor = decode_cursor(c)?;
            cursor.check_query("threads", &fingerprint)?;
            Some(cursor.after_as::<After>()?)
        }
        None => None,
    };
    let latest_seq = ctx
        .core
        .store
        .read(latest_seq)
        .map_err(|e| ApiError::from(polygloss_core::review::CoreError::from(e)))?;
    let all = query.threads(ctx)?;
    let start = match &after {
        None => 0,
        Some(a) => match all.iter().position(|t| t.id == a.thread_id) {
            Some(i) => i + 1,
            None => all
                .iter()
                .position(|t| t.created_at > a.created_at)
                .unwrap_or(all.len()),
        },
    };
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let rest = &all[start.min(all.len())..];
    let page: Vec<&ThreadView> = rest.iter().take(limit).collect();
    let more = rest.len() > page.len();
    let summaries = query.summaries(ctx, &page)?;
    let (threads, cut) = fit_page(summaries, PAGE_BUDGET);
    let next_cursor = match threads.last() {
        Some(_) if cut || more => {
            let last = page[threads.len() - 1];
            let after = After {
                created_at: last.created_at,
                thread_id: last.id.clone(),
            };
            Some(encode_cursor(&Cursor::new("threads", fingerprint, &after)?))
        }
        _ => None,
    };
    Ok(ListThreadsResult {
        threads,
        next_cursor,
        latest_seq,
    })
}
