//! Agent sessions, review assignments and waiters (T1.14, design §16.3, §16.4,
//! OQ-32), plus the per-review `last_seen_seq` and mute flags.
//!
//! Rules:
//!
//! - **Canonical ids** (session-id drift, §16.4, Provisional): a session seen for
//!   the first time, or seen with a new `owner_pid`, is linked to the canonical
//!   session of the earliest-seen other session with the same owner pid
//!   (`sessions.canonical_id` always points at a root, a row whose own
//!   `canonical_id` is `NULL`). Sessions without an owner pid are never linked.
//!   [`Core::canonical_session`] follows the link; an unknown id is its own
//!   canonical id.
//! - **Assignments** are stored against the canonical session: one per review,
//!   latest opener wins (§15.2 `open_diff`, OQ-32). Lookups by session cover every
//!   id linked to the same root. A change appends `review.assigned`
//!   (`{session_id, assigned_by}`); assigning the same session again by the same
//!   means writes nothing.
//! - **Open** for the waiter (§16.3 step 2) means not archived and not approved
//!   (`approve` means done, §15.2).
//! - **Waiters**: one row per canonical session; a newer waiter replaces the older
//!   one and gets its pid back to signal. A waiter only removes its own row. A
//!   waiter counts as live while its pid exists (probed with `/bin/kill -0`, since
//!   core has no `libc` and denies `unsafe`; waiters run as the same user) and its
//!   deadline has not passed.
//! - Sessions, waiters, `last_seen_seq` and mute have no event kind (§7.3), so
//!   those writes append none.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::review::models::path_to_db;
use crate::review::viewed::review_exists;
use crate::review::{Core, CoreError};
use crate::store::StoreError;
use crate::store::events::{Actor, ActorKind, EventKind, NewEvent, append_event, now_ms};

/// How many `canonical_id` links [`Core::canonical_session`] follows before it
/// stops (links always point at a root, so one is the norm).
const MAX_LINK_DEPTH: usize = 16;

/// SQL (bound `?1` = a canonical session id) for the ids linked to it, itself
/// included.
const SESSION_GROUP_SQL: &str = "WITH RECURSIVE grp(id) AS (\
       SELECT ?1 UNION SELECT s.id FROM sessions s JOIN grp ON s.canonical_id = grp.id) \
     SELECT id FROM grp";

/// An agent session (design §7.2 `sessions`): `CLAUDE_CODE_SESSION_ID` or
/// `pg-<uuidv7>`, named by the MCP `clientInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// MCP `clientInfo.name` (`claude-code`), or `cli`.
    pub client_name: String,
    pub client_version: Option<String>,
    /// The agent host process that owns this session (§16.4, Provisional).
    pub owner_pid: Option<i32>,
    pub cwd: Option<PathBuf>,
}

/// Who assigned a review to a session (`review_assignments.assigned_by`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignedBy {
    /// An agent's `open_diff` with `assign = true`.
    OpenDiff,
    /// The human's "Assign to session…" (OQ-32).
    Human,
    /// Any other agent-initiated assignment.
    Agent,
}

impl AssignedBy {
    /// The `review_assignments.assigned_by` value.
    pub fn as_str(&self) -> &'static str {
        match self {
            AssignedBy::OpenDiff => "open_diff",
            AssignedBy::Human => "human",
            AssignedBy::Agent => "agent",
        }
    }
}

impl Core {
    /// Records `s` (inserting it, or updating its client, pid, cwd and
    /// `last_seen_at`) and returns its canonical id, linking it by owner pid
    /// (module docs).
    pub fn upsert_session(&self, s: &SessionInfo) -> Result<String, CoreError> {
        let now = now_ms();
        let cwd = s.cwd.as_deref().map(path_to_db);
        Ok(self.store.write(|tx| {
            let existing: Option<Option<i32>> = tx
                .query_row(
                    "SELECT owner_pid FROM sessions WHERE id = ?1",
                    [&s.id],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                None => {
                    tx.execute(
                        "INSERT INTO sessions (id, client_name, client_version, owner_pid, cwd, \
                           first_seen_at, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                        params![s.id, s.client_name, s.client_version, s.owner_pid, cwd, now],
                    )?;
                }
                Some(_) => {
                    tx.execute(
                        "UPDATE sessions SET client_name = ?2, client_version = ?3, \
                           owner_pid = ?4, cwd = COALESCE(?5, cwd), \
                           last_seen_at = MAX(last_seen_at, ?6) WHERE id = ?1",
                        params![s.id, s.client_name, s.client_version, s.owner_pid, cwd, now],
                    )?;
                }
            }
            let pid_changed = existing.is_none_or(|old| old != s.owner_pid);
            if pid_changed && let Some(pid) = s.owner_pid {
                link_by_owner_pid(tx, &s.id, pid)?;
            }
            canonical(tx, &s.id)
        })?)
    }

    /// The canonical id of session `id` (itself when unlinked or unknown).
    pub fn canonical_session(&self, id: &str) -> Result<String, CoreError> {
        Ok(self.store.read(|c| canonical(c, id))?)
    }

    /// Assigns the review to the canonical session of `session_id`, replacing any
    /// earlier assignment (latest opener wins). `NotFound` for an unknown review
    /// or session.
    pub fn assign_review(
        &self,
        review_id: &str,
        session_id: &str,
        by: AssignedBy,
    ) -> Result<(), CoreError> {
        self.store.write(|tx| {
            if !review_exists(tx, review_id)? {
                return Ok(Err(CoreError::not_found("review", review_id)));
            }
            let Some((client_name,)) = session_row(tx, session_id)? else {
                return Ok(Err(CoreError::not_found("session", session_id)));
            };
            let session = canonical(tx, session_id)?;
            let current: Option<(String, String)> = tx
                .query_row(
                    "SELECT session_id, assigned_by FROM review_assignments WHERE review_id = ?1",
                    [review_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if current.is_some_and(|(s, b)| s == session && b == by.as_str()) {
                return Ok(Ok(()));
            }
            tx.execute(
                "INSERT INTO review_assignments (review_id, session_id, assigned_at, assigned_by) \
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT (review_id) DO UPDATE SET \
                   session_id = excluded.session_id, assigned_at = excluded.assigned_at, \
                   assigned_by = excluded.assigned_by",
                params![review_id, session, now_ms(), by.as_str()],
            )?;
            let actor = match by {
                AssignedBy::Human => Actor::human(),
                AssignedBy::OpenDiff | AssignedBy::Agent => Actor {
                    kind: ActorKind::Agent,
                    name: Some(client_name),
                    session_id: Some(session_id.to_owned()),
                },
            };
            append_event(
                tx,
                &NewEvent {
                    kind: EventKind::ReviewAssigned,
                    review_id: Some(review_id.to_owned()),
                    diff_id: None,
                    thread_id: None,
                    comment_id: None,
                    actor,
                    payload: json!({ "session_id": session, "assigned_by": by.as_str() }),
                },
            )?;
            Ok(Ok(()))
        })?
    }

    /// Canonical sessions active since `seen_since_ms` (their own or a linked id's
    /// `last_seen_at`), most recently seen first: the "Assign to session…" list
    /// (OQ-32, 7 days).
    pub fn recent_sessions(&self, seen_since_ms: i64) -> Result<Vec<SessionInfo>, CoreError> {
        Ok(self.store.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT s.id, s.client_name, s.client_version, s.owner_pid, s.cwd, \
                   (SELECT MAX(a.last_seen_at) FROM sessions a \
                    WHERE a.id = s.id OR a.canonical_id = s.id) AS seen \
                 FROM sessions s WHERE s.canonical_id IS NULL AND seen >= ?1 \
                 ORDER BY seen DESC, s.id",
            )?;
            let rows = stmt
                .query_map([seen_since_ms], |r| {
                    Ok(SessionInfo {
                        id: r.get(0)?,
                        client_name: r.get(1)?,
                        client_version: r.get(2)?,
                        owner_pid: r.get(3)?,
                        cwd: r
                            .get::<_, Option<String>>(4)?
                            .map(|p| crate::review::models::path_from_db(&p)),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?)
    }

    /// The open (not archived, not approved) reviews assigned to the session or
    /// any id linked to its canonical session, most recently updated first.
    pub fn assigned_open_reviews(&self, session_id: &str) -> Result<Vec<String>, CoreError> {
        Ok(self.store.read(|c| {
            let root = canonical(c, session_id)?;
            let mut stmt = c.prepare_cached(&format!(
                "SELECT r.id FROM review_assignments a JOIN reviews r ON r.id = a.review_id \
                 WHERE a.session_id IN ({SESSION_GROUP_SQL}) \
                   AND r.archived_at IS NULL AND r.status <> 'approved' \
                 ORDER BY r.updated_at DESC, r.id"
            ))?;
            let ids = stmt
                .query_map([&root], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(ids)
        })?)
    }

    /// Registers `pid` as the waiter of the session's canonical session, replacing
    /// the previous one. Returns the replaced pid (for the caller to signal after
    /// checking it really is a waiter), `None` when there was none or it was
    /// `pid`. `NotFound` for an unknown session.
    pub fn register_waiter(
        &self,
        session_id: &str,
        pid: i32,
        deadline_at: i64,
    ) -> Result<Option<i32>, CoreError> {
        self.store.write(|tx| {
            if session_row(tx, session_id)?.is_none() {
                return Ok(Err(CoreError::not_found("session", session_id)));
            }
            let session = canonical(tx, session_id)?;
            let previous: Option<i32> = tx
                .query_row(
                    "SELECT pid FROM waiters WHERE session_id = ?1",
                    [&session],
                    |r| r.get(0),
                )
                .optional()?;
            tx.execute(
                "INSERT INTO waiters (session_id, pid, started_at, deadline_at) \
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT (session_id) DO UPDATE SET \
                   pid = excluded.pid, started_at = excluded.started_at, \
                   deadline_at = excluded.deadline_at",
                params![session, pid, now_ms(), deadline_at],
            )?;
            Ok(Ok(previous.filter(|&p| p != pid)))
        })?
    }

    /// Removes the session's waiter row if it is still `pid`'s (a replaced waiter
    /// exiting leaves its replacement alone).
    pub fn remove_waiter(&self, session_id: &str, pid: i32) -> Result<(), CoreError> {
        self.store.write(|tx| {
            let session = canonical(tx, session_id)?;
            tx.execute(
                "DELETE FROM waiters WHERE session_id = ?1 AND pid = ?2",
                params![session, pid],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// The session the review is assigned to (its canonical session), `None`
    /// when unassigned. `NotFound` for an unknown review.
    pub fn assigned_session(&self, review_id: &str) -> Result<Option<SessionInfo>, CoreError> {
        let found: Option<Option<SessionInfo>> = self.store.read(|c| {
            if !review_exists(c, review_id)? {
                return Ok(None);
            }
            let session: Option<String> = c
                .query_row(
                    "SELECT session_id FROM review_assignments WHERE review_id = ?1",
                    [review_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(session) = session else {
                return Ok(Some(None));
            };
            let root = canonical(c, &session)?;
            Ok(Some(
                c.query_row(
                    "SELECT id, client_name, client_version, owner_pid, cwd FROM sessions \
                     WHERE id = ?1",
                    [&root],
                    |r| {
                        Ok(SessionInfo {
                            id: r.get(0)?,
                            client_name: r.get(1)?,
                            client_version: r.get(2)?,
                            owner_pid: r.get(3)?,
                            cwd: r
                                .get::<_, Option<String>>(4)?
                                .map(|p| crate::review::models::path_from_db(&p)),
                        })
                    },
                )
                .optional()?,
            ))
        })?;
        found.ok_or_else(|| CoreError::not_found("review", review_id))
    }

    /// The live waiter `(canonical session id, pid)` of the session the review is
    /// assigned to, if any. Dead pids and passed deadlines count as absent.
    /// `NotFound` for an unknown review.
    pub fn live_waiter_for_review(
        &self,
        review_id: &str,
    ) -> Result<Option<(String, i32)>, CoreError> {
        let waiter: Option<Option<(String, i32, i64)>> = self.store.read(|c| {
            if !review_exists(c, review_id)? {
                return Ok(None);
            }
            let session: Option<String> = c
                .query_row(
                    "SELECT session_id FROM review_assignments WHERE review_id = ?1",
                    [review_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(session) = session else {
                return Ok(Some(None));
            };
            let root = canonical(c, &session)?;
            Ok(Some(
                c.query_row(
                    &format!(
                        "SELECT session_id, pid, deadline_at FROM waiters \
                         WHERE session_id IN ({SESSION_GROUP_SQL}) \
                         ORDER BY started_at DESC LIMIT 1"
                    ),
                    [&root],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?,
            ))
        })?;
        let Some(waiter) = waiter else {
            return Err(CoreError::not_found("review", review_id));
        };
        Ok(waiter
            .filter(|(_, pid, deadline)| *deadline > now_ms() && pid_alive(*pid))
            .map(|(session, pid, _)| (session, pid)))
    }

    /// Records that the human has seen the review's events up to `seq` (never
    /// moves backwards). `NotFound` for an unknown review.
    pub fn mark_seen(&self, review_id: &str, seq: i64) -> Result<(), CoreError> {
        self.update_review(
            review_id,
            "UPDATE reviews SET last_seen_seq = MAX(last_seen_seq, ?2) WHERE id = ?1",
            seq,
        )
    }

    /// Mutes or unmutes the review's notifications (design §17). `NotFound` for
    /// an unknown review.
    pub fn set_muted(&self, review_id: &str, muted: bool) -> Result<(), CoreError> {
        self.update_review(
            review_id,
            "UPDATE reviews SET muted = ?2 WHERE id = ?1",
            i64::from(muted),
        )
    }

    fn update_review(&self, review_id: &str, sql: &str, value: i64) -> Result<(), CoreError> {
        let n = self
            .store
            .write(|tx| Ok(tx.execute(sql, params![review_id, value])?))?;
        if n == 0 {
            return Err(CoreError::not_found("review", review_id));
        }
        Ok(())
    }
}

/// `(client_name,)` of a session row.
fn session_row(conn: &Connection, id: &str) -> Result<Option<(String,)>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT client_name FROM sessions WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?,)),
        )
        .optional()?)
}

/// Follows `canonical_id` links from `id` (an unknown id is its own root).
fn canonical(conn: &Connection, id: &str) -> Result<String, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT canonical_id FROM sessions WHERE id = ?1")?;
    let mut current = id.to_owned();
    for _ in 0..MAX_LINK_DEPTH {
        let next: Option<String> = stmt
            .query_row([&current], |r| r.get(0))
            .optional()?
            .flatten();
        match next {
            Some(next) if next != current && next != id => current = next,
            _ => return Ok(current),
        }
    }
    tracing::warn!(session = id, "session link chain too long; stopping");
    Ok(current)
}

/// Links session `id` to the root of the earliest-seen other session with owner
/// `pid`, unless that root is `id` itself.
fn link_by_owner_pid(tx: &Transaction, id: &str, pid: i32) -> Result<(), StoreError> {
    let other: Option<String> = tx
        .query_row(
            "SELECT id FROM sessions WHERE owner_pid = ?1 AND id <> ?2 \
             ORDER BY first_seen_at, id LIMIT 1",
            params![pid, id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(other) = other else {
        return Ok(());
    };
    let root = canonical(tx, &other)?;
    if root == id {
        return Ok(());
    }
    tx.execute(
        "UPDATE sessions SET canonical_id = ?2 WHERE id = ?1",
        params![id, root],
    )?;
    // Keep every link pointing at a root: ids linked to `id` move to its root.
    tx.execute(
        "UPDATE sessions SET canonical_id = ?2 WHERE canonical_id = ?1",
        params![id, root],
    )?;
    Ok(())
}

/// Whether a process with this pid exists (`kill -0`; same-user waiters only).
fn pid_alive(pid: i32) -> bool {
    // `kill -0 0` or a negative pid would address process groups.
    if pid <= 0 {
        return false;
    }
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
