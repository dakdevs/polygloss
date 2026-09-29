//! Review summaries for Home and `list_reviews` (T1.14, design §11.2, §15.2, §17).
//!
//! One row per review, most recent activity (`updated_at`) first, archived reviews
//! excluded unless asked for. Counts are what every viewer may see, so the same
//! summary serves the app and agents:
//!
//! - `iterations`, `latest_diff_id`: the review's iterations (an unpinned live
//!   review has none).
//! - `viewed_done` / `viewed_total`: files of the latest iteration's diff whose
//!   Viewed key is marked (global scope, §9); `0 / 0` without an iteration.
//! - `open_threads`: open threads of the review with at least one published
//!   comment (a thread whose root is still a human draft is invisible to agents
//!   and not counted; drafts are counted by `Core::drafts_count`).
//!   `open_questions`: the open agent questions among them.
//! - `awaiting_you`: re-review requested, or an open agent question without a
//!   published, undeleted human reply (shared with `prune_stale`).
//! - `last_submission`: the newest `review_submissions` row; `rereview`: the
//!   summary and time while the status is `rereview_requested`.
//! - `assigned_session`: the canonical session of the review's assignment.

use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::{Row, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::git::ReviewKind;
use crate::ids::DiffId;
use crate::review::models::{AWAITING_YOU_SQL, path_from_db, path_to_db};
use crate::review::{Core, CoreError};
use crate::store::StoreError;

/// How many summaries one call returns when [`ReviewFilter::limit`] is `None`.
pub const DEFAULT_SUMMARY_LIMIT: u32 = 500;

/// One review row for Home and `list_reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub review_id: String,
    pub key: String,
    pub label: Option<String>,
    pub kind: ReviewKind,
    /// `repos.display_name`.
    pub repo_display: String,
    /// The repo's main worktree (its common dir without the trailing `.git`), or
    /// the common dir of a bare repo.
    pub repo_path: PathBuf,
    /// `reviews.status`: `open`, `changes_requested`, `commented`, `approved` or
    /// `rereview_requested`.
    pub status: String,
    pub iterations: u32,
    pub latest_diff_id: Option<DiffId>,
    pub viewed_done: u32,
    pub viewed_total: u32,
    pub open_threads: u32,
    pub open_questions: u32,
    pub awaiting_you: bool,
    pub last_submission: Option<SubmissionSummary>,
    /// `(summary, at)` while a re-review is requested.
    pub rereview: Option<(String, i64)>,
    pub assigned_session: Option<String>,
    pub muted: bool,
    pub updated_at: i64,
}

impl ReviewSummary {
    /// The keyset cursor that continues after this row ([`ReviewFilter::after`]).
    pub fn cursor(&self) -> SummaryCursor {
        SummaryCursor {
            updated_at: self.updated_at,
            review_id: self.review_id.clone(),
        }
    }
}

/// The latest submission of a review (`list_reviews.last_submission`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionSummary {
    pub submission_id: String,
    /// `request_changes`, `comment` or `approve`.
    pub verdict: String,
    pub summary_md: String,
    /// `submitted_at`, Unix ms.
    pub at: i64,
}

/// A position in the `(updated_at DESC, review_id DESC)` order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SummaryCursor {
    pub updated_at: i64,
    pub review_id: String,
}

/// Which summaries [`Core::review_summaries`] returns. The default is every
/// unarchived review, newest first, up to [`DEFAULT_SUMMARY_LIMIT`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewFilter {
    /// Only reviews of this repo (its realpath'd git common dir, `RepoInfo::common_dir`).
    pub repo_common_dir: Option<PathBuf>,
    /// Only reviews with this `reviews.status`.
    pub status: Option<String>,
    /// Only reviews assigned to this session (or any id linked to its canonical
    /// session): `list_reviews(assigned = "me")`.
    pub assigned_session: Option<String>,
    /// Include archived reviews.
    pub include_archived: bool,
    /// Continue after this row (keyset pagination).
    pub after: Option<SummaryCursor>,
    /// At most this many rows (`None` = [`DEFAULT_SUMMARY_LIMIT`]).
    pub limit: Option<u32>,
}

impl Core {
    /// Review summaries matching `filter`, most recently active first (module docs).
    pub fn review_summaries(&self, filter: &ReviewFilter) -> Result<Vec<ReviewSummary>, CoreError> {
        let assigned = filter
            .assigned_session
            .as_deref()
            .map(|s| self.canonical_session(s))
            .transpose()?;
        let mut sql = format!(
            "WITH latest AS (\
               SELECT i.review_id, i.diff_id, \
                 ROW_NUMBER() OVER (PARTITION BY i.review_id ORDER BY i.seq DESC) AS rn, \
                 COUNT(*) OVER (PARTITION BY i.review_id) AS n \
               FROM iterations i) \
             SELECT r.id, r.key, r.label, r.kind, p.display_name, p.common_dir, r.status, \
               COALESCE(l.n, 0), l.diff_id, \
               (SELECT COUNT(*) FROM file_changes f WHERE f.diff_id = l.diff_id AND EXISTS (\
                  SELECT 1 FROM viewed_files v WHERE v.path = COALESCE(f.new_path, f.old_path) \
                  AND v.old_blob = f.old_blob AND v.new_blob = f.new_blob)), \
               (SELECT COUNT(*) FROM file_changes f WHERE f.diff_id = l.diff_id), \
               (SELECT COUNT(*) FROM threads t WHERE t.review_id = r.id AND t.status = 'open' \
                  AND EXISTS (SELECT 1 FROM comments c WHERE c.thread_id = t.id \
                    AND c.published_at IS NOT NULL)), \
               (SELECT COUNT(*) FROM threads t WHERE t.review_id = r.id AND t.status = 'open' \
                  AND t.kind = 'question'), \
               {AWAITING_YOU_SQL}, \
               s.id, s.verdict, s.summary_md, s.submitted_at, \
               r.rereview_summary, r.rereview_at, a.session_id, r.muted, r.updated_at \
             FROM reviews r JOIN repos p ON p.id = r.repo_id \
             LEFT JOIN latest l ON l.review_id = r.id AND l.rn = 1 \
             LEFT JOIN review_assignments a ON a.review_id = r.id \
             LEFT JOIN review_submissions s ON s.id = (\
               SELECT s2.id FROM review_submissions s2 WHERE s2.review_id = r.id \
               ORDER BY s2.submitted_at DESC, s2.id DESC LIMIT 1) \
             WHERE 1 = 1"
        );
        let mut args: Vec<Value> = Vec::new();
        let mut arg = |sql: &mut String, clause: &str, value: Value| {
            args.push(value);
            sql.push_str(&clause.replace('?', &format!("?{}", args.len())));
        };
        if !filter.include_archived {
            sql.push_str(" AND r.archived_at IS NULL");
        }
        if let Some(dir) = &filter.repo_common_dir {
            arg(
                &mut sql,
                " AND p.common_dir = ?",
                Value::Text(path_to_db(dir)),
            );
        }
        if let Some(status) = &filter.status {
            arg(&mut sql, " AND r.status = ?", Value::Text(status.clone()));
        }
        if let Some(root) = assigned {
            arg(
                &mut sql,
                " AND a.session_id IN (WITH RECURSIVE grp(id) AS (SELECT ? UNION \
                   SELECT x.id FROM sessions x JOIN grp ON x.canonical_id = grp.id) \
                   SELECT id FROM grp)",
                Value::Text(root),
            );
        }
        if let Some(after) = &filter.after {
            arg(
                &mut sql,
                " AND (r.updated_at < ?",
                Value::Integer(after.updated_at),
            );
            arg(
                &mut sql,
                " OR (r.updated_at = ?",
                Value::Integer(after.updated_at),
            );
            arg(
                &mut sql,
                " AND r.id < ?))",
                Value::Text(after.review_id.clone()),
            );
        }
        let limit = filter.limit.unwrap_or(DEFAULT_SUMMARY_LIMIT);
        arg(
            &mut sql,
            " ORDER BY r.updated_at DESC, r.id DESC LIMIT ?",
            Value::Integer(i64::from(limit)),
        );
        Ok(self.store.read(|c| {
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(params_from_iter(args.iter()), |r| Ok(read_summary(r)))?
                .map(|r| r.map_err(StoreError::from).and_then(|x| x))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?)
    }
}

fn read_summary(r: &Row) -> Result<ReviewSummary, StoreError> {
    let kind: String = r.get(3)?;
    let latest: Option<String> = r.get(8)?;
    let status: String = r.get(6)?;
    let submission = match r.get::<_, Option<String>>(14)? {
        Some(submission_id) => Some(SubmissionSummary {
            submission_id,
            verdict: r.get(15)?,
            summary_md: r.get(16)?,
            at: r.get(17)?,
        }),
        None => None,
    };
    let rereview = match (
        r.get::<_, Option<String>>(18)?,
        r.get::<_, Option<i64>>(19)?,
    ) {
        (Some(summary), Some(at)) if status == "rereview_requested" => Some((summary, at)),
        _ => None,
    };
    Ok(ReviewSummary {
        review_id: r.get(0)?,
        key: r.get(1)?,
        label: r.get(2)?,
        kind: parse_kind(&kind)
            .ok_or_else(|| StoreError::Integrity(format!("review kind {kind:?}")))?,
        repo_display: r.get(4)?,
        repo_path: repo_path(&path_from_db(&r.get::<_, String>(5)?)),
        status,
        iterations: r.get(7)?,
        latest_diff_id: latest
            .map(|d| DiffId::parse(&d).map_err(|_| StoreError::Integrity(format!("diff id {d:?}"))))
            .transpose()?,
        viewed_done: r.get(9)?,
        viewed_total: r.get(10)?,
        open_threads: r.get(11)?,
        open_questions: r.get(12)?,
        awaiting_you: r.get(13)?,
        last_submission: submission,
        rereview,
        assigned_session: r.get(20)?,
        muted: r.get(21)?,
        updated_at: r.get(22)?,
    })
}

fn parse_kind(s: &str) -> Option<ReviewKind> {
    [ReviewKind::Live, ReviewKind::Compare, ReviewKind::Commit]
        .into_iter()
        .find(|k| k.as_str() == s)
}

/// The main worktree of a repo from its common dir: `<wt>/.git` → `<wt>`; a bare
/// repo's common dir is itself.
fn repo_path(common_dir: &Path) -> PathBuf {
    match common_dir.file_name() {
        Some(name) if name == ".git" => common_dir
            .parent()
            .map_or_else(|| common_dir.to_path_buf(), Path::to_path_buf),
        _ => common_dir.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_path_strips_dot_git_only() {
        assert_eq!(repo_path(Path::new("/src/app/.git")), Path::new("/src/app"));
        assert_eq!(
            repo_path(Path::new("/srv/app.git")),
            Path::new("/srv/app.git")
        );
    }

    #[test]
    fn kinds_parse() {
        for k in [ReviewKind::Live, ReviewKind::Compare, ReviewKind::Commit] {
            assert_eq!(parse_kind(k.as_str()), Some(k));
        }
        assert_eq!(parse_kind("pr"), None);
    }
}
