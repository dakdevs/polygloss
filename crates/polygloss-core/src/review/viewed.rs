//! Viewed state keyed by `(path, old_blob, new_blob)` (T1.14, design §9).
//!
//! Rules (ADR-0022, OQ-8):
//!
//! - The key is [`FileChange::viewed_key`]: the display path (new path, else old
//!   path, in the store's TEXT form) and both blob ids. Added files carry the
//!   all-zero `old_blob`, deleted files the all-zero `new_blob`.
//! - Scope is global: a key marked in any review (or without one) is viewed
//!   everywhere, and it carries over across iterations while the key is unchanged.
//!   `viewed_files.review_id` only records where it was marked.
//! - "Changed since viewed": no row for the exact key, but a row for the same
//!   `(review_id, path)` with another blob pair.
//! - Unmarking deletes the exact key and detaches (`review_id = NULL`) the
//!   review's other rows for that path, so an explicit "not viewed" clears the
//!   badge while older pairs stay viewed globally.
//! - Never pins: blob ids exist for unpinned live states too. A change appends
//!   `viewed.changed` (human, not agent-visible) and bumps the review's
//!   `updated_at`; setting the state it already has writes nothing.

use polygloss_diff::FileChange;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::review::{Core, CoreError};
use crate::store::StoreError;
use crate::store::events::{Actor, EventKind, NewEvent, append_event, now_ms};

/// A file's Viewed checkbox state (design §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewedState {
    NotViewed,
    Viewed,
    /// Unchecked with a "Changed since viewed" badge: this review viewed an
    /// earlier blob pair of the path.
    ChangedSinceViewed,
}

impl Core {
    /// Marks `change` viewed or not viewed, recording `review_id` as where it was
    /// marked (`None` when there is no review). Never pins or records an iteration.
    /// `NotFound` for an unknown review.
    pub fn set_viewed(
        &self,
        review_id: Option<&str>,
        change: &FileChange,
        viewed: bool,
    ) -> Result<(), CoreError> {
        let (path, old_blob, new_blob) = change.viewed_key();
        let found = self.store.write(|tx| {
            if let Some(id) = review_id
                && !review_exists(tx, id)?
            {
                return Ok(false);
            }
            let changed = if viewed {
                mark(tx, review_id, &path, old_blob.as_str(), new_blob.as_str())?
            } else {
                unmark(tx, review_id, &path, old_blob.as_str(), new_blob.as_str())?
            };
            if !changed {
                return Ok(true);
            }
            let now = now_ms();
            if let Some(id) = review_id {
                tx.execute(
                    "UPDATE reviews SET updated_at = MAX(updated_at, ?2) WHERE id = ?1",
                    params![id, now],
                )?;
            }
            append_event(
                tx,
                &NewEvent {
                    kind: EventKind::ViewedChanged,
                    review_id: review_id.map(str::to_owned),
                    diff_id: None,
                    thread_id: None,
                    comment_id: None,
                    actor: Actor::human(),
                    payload: json!({
                        "path": path,
                        "old_blob": old_blob.as_str(),
                        "new_blob": new_blob.as_str(),
                        "viewed": viewed,
                    }),
                },
            )?;
            Ok(true)
        })?;
        if found {
            Ok(())
        } else {
            Err(CoreError::not_found(
                "review",
                review_id.unwrap_or_default(),
            ))
        }
    }

    /// The Viewed state of each of `files`, in order. With `review_id`, a path this
    /// review viewed with another blob pair is [`ViewedState::ChangedSinceViewed`];
    /// without one, only `Viewed` or `NotViewed`.
    pub fn viewed_states(
        &self,
        review_id: Option<&str>,
        files: &[FileChange],
    ) -> Result<Vec<ViewedState>, CoreError> {
        Ok(self.store.read(|c| {
            let mut exact = c.prepare_cached(
                "SELECT 1 FROM viewed_files WHERE path = ?1 AND old_blob = ?2 AND new_blob = ?3",
            )?;
            let mut earlier = c.prepare_cached(
                "SELECT 1 FROM viewed_files WHERE review_id = ?1 AND path = ?2 LIMIT 1",
            )?;
            let mut out = Vec::with_capacity(files.len());
            for f in files {
                let (path, old_blob, new_blob) = f.viewed_key();
                let state = if exact.exists(params![path, old_blob.as_str(), new_blob.as_str()])? {
                    ViewedState::Viewed
                } else if let Some(id) = review_id
                    && earlier.exists(params![id, path])?
                {
                    ViewedState::ChangedSinceViewed
                } else {
                    ViewedState::NotViewed
                };
                out.push(state);
            }
            Ok(out)
        })?)
    }
}

/// Inserts the key (or moves it to `review_id`). Returns whether anything changed.
fn mark(
    tx: &Transaction,
    review_id: Option<&str>,
    path: &str,
    old_blob: &str,
    new_blob: &str,
) -> Result<bool, StoreError> {
    let existing: Option<Option<String>> = tx
        .query_row(
            "SELECT review_id FROM viewed_files WHERE path = ?1 AND old_blob = ?2 AND new_blob = ?3",
            params![path, old_blob, new_blob],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        // Already viewed: keep where it was first marked; a key marked without a
        // review is claimed by the first review that marks it.
        Some(Some(_)) => Ok(false),
        Some(None) if review_id.is_none() => Ok(false),
        Some(None) => {
            tx.execute(
                "UPDATE viewed_files SET review_id = ?4 \
                 WHERE path = ?1 AND old_blob = ?2 AND new_blob = ?3",
                params![path, old_blob, new_blob, review_id],
            )?;
            Ok(true)
        }
        None => {
            tx.execute(
                "INSERT INTO viewed_files (path, old_blob, new_blob, review_id, viewed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![path, old_blob, new_blob, review_id, now_ms()],
            )?;
            Ok(true)
        }
    }
}

/// Deletes the key and detaches the review's other rows for the path. Returns
/// whether anything changed.
fn unmark(
    tx: &Transaction,
    review_id: Option<&str>,
    path: &str,
    old_blob: &str,
    new_blob: &str,
) -> Result<bool, StoreError> {
    let deleted = tx.execute(
        "DELETE FROM viewed_files WHERE path = ?1 AND old_blob = ?2 AND new_blob = ?3",
        params![path, old_blob, new_blob],
    )? > 0;
    let detached = match review_id {
        Some(id) => tx.execute(
            "UPDATE viewed_files SET review_id = NULL WHERE review_id = ?1 AND path = ?2",
            params![id, path],
        )?,
        None => 0,
    };
    Ok(deleted || detached > 0)
}

pub(crate) fn review_exists(
    conn: &rusqlite::Connection,
    review_id: &str,
) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM reviews WHERE id = ?1")?
        .exists([review_id])?)
}
