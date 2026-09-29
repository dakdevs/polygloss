//! What agents did on a review since the human last looked (T3.13, design
//! §8.3 step 5, §11.7, §17): the threads with agent events after
//! `reviews.last_seen_seq` ("claude-code replied to N threads") and a
//! pending re-review request ("Re-review requested" with its summary).
//!
//! The app's store feed reads this when a review tab opens and again after
//! store events of that review; [`Core::mark_seen`] advances the watermark as
//! the human visits the threads.

use rusqlite::{OptionalExtension, params};

use crate::review::{Core, CoreError};

/// The event kinds that count as agent activity on a thread: a new thread
/// (note, question or comment), a reply, an edit or delete, a resolve or
/// unresolve (design §17 "Agent reply, resolve or note").
const THREAD_KINDS: &str = "'thread.created', 'comment.created', 'comment.edited', \
                            'comment.deleted', 'thread.resolved', 'thread.unresolved'";

/// A thread with agent events the human has not seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadThread {
    pub thread_id: String,
    /// The `seq` of the thread's latest unseen agent event. Marking the review
    /// seen up to it ([`Core::mark_seen`]) marks this thread read.
    pub last_seq: i64,
    /// The agent of that latest event (`claude-code`).
    pub actor_name: Option<String>,
}

/// A pending re-review request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rereview {
    /// The agent's markdown summary.
    pub summary: String,
    /// Unix ms.
    pub at: i64,
    /// The agent that asked (the latest `review.rereview_requested` event).
    pub requested_by: Option<String>,
}

/// Agent activity on one review the human has not dealt with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewActivity {
    /// `reviews.last_seen_seq`: the human has seen events up to here.
    pub last_seen_seq: i64,
    /// Threads of the review with agent events after `last_seen_seq`,
    /// ordered by their latest such event (oldest first), so visiting them
    /// in order and marking each one's `last_seq` seen never marks an
    /// unvisited thread read. Threads that no longer exist are left out.
    pub unread: Vec<UnreadThread>,
    /// Set while the review's status is `rereview_requested`.
    pub rereview: Option<Rereview>,
}

impl Core {
    /// The review's unseen agent activity and pending re-review request.
    /// `NotFound` for an unknown review.
    pub fn review_activity(&self, review_id: &str) -> Result<ReviewActivity, CoreError> {
        let activity = self.store.read(|c| {
            let row: Option<(i64, String, Option<String>, Option<i64>)> = c
                .query_row(
                    "SELECT last_seen_seq, status, rereview_summary, rereview_at \
                     FROM reviews WHERE id = ?1",
                    [review_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((last_seen_seq, status, summary, at)) = row else {
                return Ok(None);
            };
            // SQLite's bare columns with `MAX()`: `actor_name` comes from the
            // row holding the latest `seq` of each thread.
            let mut stmt = c.prepare_cached(&format!(
                "SELECT thread_id, MAX(seq), actor_name FROM events \
                 WHERE review_id = ?1 AND seq > ?2 AND actor_kind = 'agent' \
                   AND thread_id IS NOT NULL AND kind IN ({THREAD_KINDS}) \
                   AND thread_id IN (SELECT id FROM threads) \
                 GROUP BY thread_id ORDER BY MAX(seq)"
            ))?;
            let unread = stmt
                .query_map(params![review_id, last_seen_seq], |r| {
                    Ok(UnreadThread {
                        thread_id: r.get(0)?,
                        last_seq: r.get(1)?,
                        actor_name: r.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let rereview = if status == "rereview_requested" {
                let requested_by: Option<String> = c
                    .query_row(
                        "SELECT actor_name FROM events \
                         WHERE review_id = ?1 AND kind = 'review.rereview_requested' \
                         ORDER BY seq DESC LIMIT 1",
                        [review_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                Some(Rereview {
                    summary: summary.unwrap_or_default(),
                    at: at.unwrap_or(0),
                    requested_by,
                })
            } else {
                None
            };
            Ok(Some(ReviewActivity {
                last_seen_seq,
                unread,
                rereview,
            }))
        })?;
        activity.ok_or_else(|| CoreError::not_found("review", review_id))
    }
}
