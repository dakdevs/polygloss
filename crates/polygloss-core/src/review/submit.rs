//! Submit review and request re-review (T1.13, design §8.3, ADR-0011).
//!
//! Submitting pins a live state first (design §5.2), then one `BEGIN IMMEDIATE`
//! transaction publishes every draft of the review, writes `review_submissions`,
//! sets `reviews.status`, clears the autosaved dialog, and appends
//! `thread.created` (a published draft root) / `comment.created` (a published
//! draft reply) in draft order, then `review.submitted` last. Nothing interleaves,
//! so those events have contiguous seqs.
//!
//! Live states are passed with the base the caller displayed
//! (`OpenedDiff.{base, live}`): a `LiveState` carries no base, and re-resolving it
//! (for `since=merge-base`, after the default branch moved) could pin a diff the
//! human never saw. Pinning goes through [`Core::pin_live_on_base`].

use rusqlite::{OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::git::{LiveState, ResolvedSide};
use crate::ids::new_uuid;
use crate::review::models::{IterationInfo, PinnedBy};
use crate::review::open::{latest_iteration, review_exists};
use crate::review::threads::{draft_event, thread_event};
use crate::review::{Core, CoreError};
use crate::store::events::{Actor, ActorKind, EventKind, NewEvent, append_event, now_ms};

/// The Submit review verdict (`review_submissions.verdict`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    RequestChanges,
    Comment,
    Approve,
}

impl Verdict {
    /// `request_changes` | `comment` | `approve`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::RequestChanges => "request_changes",
            Verdict::Comment => "comment",
            Verdict::Approve => "approve",
        }
    }

    pub fn parse(s: &str) -> Option<Verdict> {
        [Verdict::RequestChanges, Verdict::Comment, Verdict::Approve]
            .into_iter()
            .find(|v| v.as_str() == s)
    }

    /// The `reviews.status` a submission with this verdict sets.
    pub fn review_status(&self) -> &'static str {
        match self {
            Verdict::RequestChanges => "changes_requested",
            Verdict::Comment => "commented",
            Verdict::Approve => "approved",
        }
    }
}

/// A recorded submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    /// `review_submissions.id` (UUIDv7).
    pub id: String,
    pub review_id: String,
    /// The iteration submitted against (pinned first for live reviews).
    pub iteration: IterationInfo,
    pub verdict: Verdict,
    pub summary_md: String,
    /// Drafts this submission published.
    pub comment_count: u32,
    pub submitted_at: i64,
    /// The seq of its `review.submitted` event.
    pub seq: i64,
}

/// The autosaved Submit review dialog (`review_drafts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitDraft {
    pub summary_md: String,
    pub verdict: Option<Verdict>,
}

/// A draft being published: `(comment id, thread id, author name, is root,
/// thread kind, thread subject, origin diff)`.
type Draft = (String, String, String, bool, String, String, String);

impl Core {
    /// Autosaves the Submit dialog (`review_drafts`) with a `draft.changed` event.
    pub fn save_submit_draft(
        &self,
        review_id: &str,
        summary_md: &str,
        verdict: Option<Verdict>,
    ) -> Result<(), CoreError> {
        self.store.write(|tx| {
            if !review_exists(tx, review_id)? {
                return Ok(Err(CoreError::not_found("review", review_id)));
            }
            tx.execute(
                "INSERT INTO review_drafts (review_id, summary_md, verdict, updated_at) \
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT (review_id) DO UPDATE SET \
                   summary_md = excluded.summary_md, verdict = excluded.verdict, \
                   updated_at = excluded.updated_at",
                params![review_id, summary_md, verdict.map(|v| v.as_str()), now_ms()],
            )?;
            append_event(
                tx,
                &draft_event(Some(review_id), None, None, "edited", "submit_dialog"),
            )?;
            Ok(Ok(()))
        })?
    }

    /// The autosaved Submit dialog of the review ([`Core::save_submit_draft`]),
    /// `None` when nothing was saved since the last submission. `NotFound` for an
    /// unknown review.
    pub fn submit_draft(&self, review_id: &str) -> Result<Option<SubmitDraft>, CoreError> {
        let row: Option<Option<(String, Option<String>)>> = self.store.read(|c| {
            if !review_exists(c, review_id)? {
                return Ok(None);
            }
            Ok(Some(
                c.query_row(
                    "SELECT summary_md, verdict FROM review_drafts WHERE review_id = ?1",
                    [review_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?,
            ))
        })?;
        let row = row.ok_or_else(|| CoreError::not_found("review", review_id))?;
        Ok(row.map(|(summary_md, verdict)| SubmitDraft {
            summary_md,
            // An unknown stored verdict reads as none chosen.
            verdict: verdict.as_deref().and_then(Verdict::parse),
        }))
    }

    /// Submits the review: pins `live` first (as `submit`) when given, then publishes
    /// every draft and records the verdict in one transaction (module docs). Without
    /// `live` the submission is against the latest iteration; a review without one
    /// (an unpinned live review) is a `Conflict`.
    pub fn submit_review(
        &self,
        review_id: &str,
        verdict: Verdict,
        summary_md: &str,
        live: Option<(&ResolvedSide, &LiveState)>,
    ) -> Result<Submission, CoreError> {
        let iteration =
            self.submission_iteration(review_id, live, PinnedBy::Submit, &Actor::human())?;
        let id = new_uuid();
        let now = now_ms();
        let (comment_count, seq) = self.store.write(|tx| {
            if !review_exists(tx, review_id)? {
                return Ok(Err(CoreError::not_found("review", review_id)));
            }
            let drafts: Vec<Draft> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT c.id, c.thread_id, c.author_name, \
                       c.id = (SELECT r.id FROM comments r WHERE r.thread_id = c.thread_id \
                               ORDER BY r.created_at, r.rowid LIMIT 1), \
                       t.kind, t.subject, t.origin_diff_id \
                     FROM comments c JOIN threads t ON t.id = c.thread_id \
                     WHERE t.review_id = ?1 AND c.published_at IS NULL AND c.deleted_at IS NULL \
                     ORDER BY c.created_at, c.rowid",
                )?;
                stmt.query_map([review_id], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })?
                .collect::<Result<_, _>>()?
            };
            let count = u32::try_from(drafts.len()).unwrap_or(u32::MAX);
            tx.execute(
                "INSERT INTO review_submissions (id, review_id, iteration_id, verdict, summary_md, \
                   comment_count, submitted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    review_id,
                    iteration.id,
                    verdict.as_str(),
                    summary_md,
                    count,
                    now
                ],
            )?;
            for (comment, thread, author_name, is_root, kind, subject, diff) in &drafts {
                tx.execute(
                    "UPDATE comments SET published_at = ?2, submission_id = ?3 WHERE id = ?1",
                    params![comment, now, id],
                )?;
                tx.execute(
                    "UPDATE threads SET updated_at = ?2 WHERE id = ?1",
                    params![thread, now],
                )?;
                let actor = Actor {
                    kind: ActorKind::Human,
                    name: Some(author_name.clone()),
                    session_id: None,
                };
                let (event_kind, payload) = if *is_root {
                    (
                        EventKind::ThreadCreated,
                        json!({ "kind": kind, "subject": subject }),
                    )
                } else {
                    (EventKind::CommentCreated, serde_json::Value::Null)
                };
                append_event(
                    tx,
                    &thread_event(
                        event_kind,
                        Some(review_id),
                        diff,
                        thread,
                        Some(comment),
                        &actor,
                        payload,
                    ),
                )?;
            }
            tx.execute(
                "UPDATE reviews SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![review_id, verdict.review_status(), now],
            )?;
            tx.execute(
                "DELETE FROM review_drafts WHERE review_id = ?1",
                [review_id],
            )?;
            let seq = append_event(
                tx,
                &NewEvent {
                    kind: EventKind::ReviewSubmitted,
                    review_id: Some(review_id.to_owned()),
                    diff_id: Some(iteration.diff_id.as_str().to_owned()),
                    thread_id: None,
                    comment_id: None,
                    actor: Actor::human(),
                    payload: json!({ "submission_id": id, "verdict": verdict.as_str() }),
                },
            )?;
            Ok(Ok((count, seq)))
        })??;
        Ok(Submission {
            id,
            review_id: review_id.to_owned(),
            iteration,
            verdict,
            summary_md: summary_md.to_owned(),
            comment_count,
            submitted_at: now,
            seq,
        })
    }

    /// Asks the human to review again: pins `live` first (as `rereview`; a new
    /// iteration unless the latest one shows the same diff) when given, then sets
    /// `status = rereview_requested`, `rereview_summary` and `rereview_at`, and
    /// appends `review.rereview_requested` with the summary. Returns the iteration
    /// to re-review (the latest one without `live`; `Conflict` when there is none).
    pub fn request_rereview(
        &self,
        review_id: &str,
        summary_md: &str,
        actor: &Actor,
        live: Option<(&ResolvedSide, &LiveState)>,
    ) -> Result<IterationInfo, CoreError> {
        let iteration = self.submission_iteration(review_id, live, PinnedBy::Rereview, actor)?;
        self.store.write(|tx| {
            if !review_exists(tx, review_id)? {
                return Ok(Err(CoreError::not_found("review", review_id)));
            }
            let now = now_ms();
            tx.execute(
                "UPDATE reviews SET status = 'rereview_requested', rereview_summary = ?2, \
                   rereview_at = ?3, updated_at = ?3 WHERE id = ?1",
                params![review_id, summary_md, now],
            )?;
            append_event(
                tx,
                &NewEvent {
                    kind: EventKind::ReviewRereviewRequested,
                    review_id: Some(review_id.to_owned()),
                    diff_id: Some(iteration.diff_id.as_str().to_owned()),
                    thread_id: None,
                    comment_id: None,
                    actor: actor.clone(),
                    payload: json!({ "summary": summary_md }),
                },
            )?;
            Ok(Ok(()))
        })??;
        Ok(iteration)
    }

    /// The iteration a submission or re-review refers to: `live` pinned on its
    /// displayed base, else the review's latest iteration.
    fn submission_iteration(
        &self,
        review_id: &str,
        live: Option<(&ResolvedSide, &LiveState)>,
        by: PinnedBy,
        actor: &Actor,
    ) -> Result<IterationInfo, CoreError> {
        if let Some((base, state)) = live {
            return self.pin_live_on_base(review_id, base, state, by, actor);
        }
        let latest = self.store.read(|c| {
            if !review_exists(c, review_id)? {
                return Ok(None);
            }
            Ok(Some(latest_iteration(c, review_id)?))
        })?;
        match latest {
            None => Err(CoreError::not_found("review", review_id)),
            Some(None) => Err(CoreError::Conflict(format!(
                "review {review_id} has no iteration yet; pass its live state to pin one"
            ))),
            Some(Some(it)) => Ok(it),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Verdict;

    #[test]
    fn verdict_roundtrips_and_maps_to_review_status() {
        for (v, status) in [
            (Verdict::RequestChanges, "changes_requested"),
            (Verdict::Comment, "commented"),
            (Verdict::Approve, "approved"),
        ] {
            assert_eq!(Verdict::parse(v.as_str()), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), v.as_str());
            assert_eq!(v.review_status(), status);
        }
        assert_eq!(Verdict::parse("lgtm"), None);
    }
}
