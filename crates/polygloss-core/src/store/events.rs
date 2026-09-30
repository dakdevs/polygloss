//! The `events` table: append in the same transaction and the `data_version` change feed (T1.11, design §7.3).
//!
//! Every mutation calls [`append_event`] inside its own `BEGIN IMMEDIATE`
//! transaction (design §7.1), so an event is visible exactly when the change it
//! describes is. `events` has no foreign keys and is never cascaded, so events
//! outlive the rows they mention (§7.4). `seq` is `AUTOINCREMENT`: strictly
//! increasing and never reused, which makes it a safe cursor for MCP `since`
//! parameters, `polygloss wait` and the app's store feed (ADR-0015).
//!
//! [`EventFeed`] is the cross-process change feed: a dedicated connection polls
//! `PRAGMA data_version` (about a microsecond, no table access) and reads `events`
//! after its cursor only when another connection has committed since the last poll.
//!
//! Rows whose `kind` this build does not know (written by a newer build) are
//! skipped by every reader instead of failing the whole read.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, Row, Transaction, params_from_iter};

use super::{Store, StoreError, bootstrap_connection};
use crate::paths::DataPaths;

/// Page size the feed uses when catching up on a backlog.
const FEED_PAGE: u32 = 500;

/// The kinds of `events` rows (design §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// `review.created`
    ReviewCreated,
    /// `review.archived`
    ReviewArchived,
    /// `iteration.created`
    IterationCreated,
    /// `thread.created` (human threads: written when a submission publishes them)
    ThreadCreated,
    /// `comment.created` (human comments: written when a submission publishes them)
    CommentCreated,
    /// `comment.edited`
    CommentEdited,
    /// `comment.deleted`
    CommentDeleted,
    /// `thread.resolved`
    ThreadResolved,
    /// `thread.unresolved`
    ThreadUnresolved,
    /// `review.submitted`
    ReviewSubmitted,
    /// `review.rereview_requested`
    ReviewRereviewRequested,
    /// `review.assigned`
    ReviewAssigned,
    /// `viewed.changed` (not seen by agents)
    ViewedChanged,
    /// `draft.changed` (app-internal, never seen by agents)
    DraftChanged,
}

impl EventKind {
    /// Every kind, in design §7.3 order.
    pub const ALL: [EventKind; 14] = [
        EventKind::ReviewCreated,
        EventKind::ReviewArchived,
        EventKind::IterationCreated,
        EventKind::ThreadCreated,
        EventKind::CommentCreated,
        EventKind::CommentEdited,
        EventKind::CommentDeleted,
        EventKind::ThreadResolved,
        EventKind::ThreadUnresolved,
        EventKind::ReviewSubmitted,
        EventKind::ReviewRereviewRequested,
        EventKind::ReviewAssigned,
        EventKind::ViewedChanged,
        EventKind::DraftChanged,
    ];

    /// The `events.kind` text, e.g. `"review.created"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::ReviewCreated => "review.created",
            EventKind::ReviewArchived => "review.archived",
            EventKind::IterationCreated => "iteration.created",
            EventKind::ThreadCreated => "thread.created",
            EventKind::CommentCreated => "comment.created",
            EventKind::CommentEdited => "comment.edited",
            EventKind::CommentDeleted => "comment.deleted",
            EventKind::ThreadResolved => "thread.resolved",
            EventKind::ThreadUnresolved => "thread.unresolved",
            EventKind::ReviewSubmitted => "review.submitted",
            EventKind::ReviewRereviewRequested => "review.rereview_requested",
            EventKind::ReviewAssigned => "review.assigned",
            EventKind::ViewedChanged => "viewed.changed",
            EventKind::DraftChanged => "draft.changed",
        }
    }

    /// Parses an `events.kind` text; `None` for kinds this build does not know.
    pub fn parse(s: &str) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Whether agents (MCP, JSON CLI, `polygloss wait`) may see this kind (§7.3):
    /// everything except `viewed.changed` and `draft.changed`.
    pub fn agent_visible(&self) -> bool {
        !matches!(self, EventKind::ViewedChanged | EventKind::DraftChanged)
    }
}

/// Who caused an event (`events.actor_kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActorKind {
    /// The person using the app or the human CLI.
    Human,
    /// An agent (MCP or JSON CLI).
    Agent,
    /// Polygloss itself (e.g. automatic pruning).
    System,
}

impl ActorKind {
    /// The `events.actor_kind` text.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActorKind::Human => "human",
            ActorKind::Agent => "agent",
            ActorKind::System => "system",
        }
    }

    /// Parses an `events.actor_kind` text.
    pub fn parse(s: &str) -> Option<ActorKind> {
        match s {
            "human" => Some(ActorKind::Human),
            "agent" => Some(ActorKind::Agent),
            "system" => Some(ActorKind::System),
            _ => None,
        }
    }
}

/// The actor recorded on an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    /// Human, agent or system.
    pub kind: ActorKind,
    /// Display name, e.g. `claude-code`.
    pub name: Option<String>,
    /// The agent session (canonical id), if any.
    pub session_id: Option<String>,
}

impl Actor {
    /// An anonymous human actor.
    pub fn human() -> Actor {
        Actor {
            kind: ActorKind::Human,
            name: None,
            session_id: None,
        }
    }

    /// The system actor.
    pub fn system() -> Actor {
        Actor {
            kind: ActorKind::System,
            name: None,
            session_id: None,
        }
    }
}

/// An event to append. `payload` is stored as JSON text; `Value::Null` is stored as
/// SQL `NULL` (design §7.3 kinds without a payload).
#[derive(Clone, Debug, PartialEq)]
pub struct NewEvent {
    /// The kind.
    pub kind: EventKind,
    /// The review the event belongs to, if any.
    pub review_id: Option<String>,
    /// The diff involved, if any.
    pub diff_id: Option<String>,
    /// The thread involved, if any.
    pub thread_id: Option<String>,
    /// The comment involved, if any.
    pub comment_id: Option<String>,
    /// Who caused it.
    pub actor: Actor,
    /// Kind-specific JSON (§7.3 "Payload" column).
    pub payload: serde_json::Value,
}

/// A stored event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Strictly increasing, never reused.
    pub seq: i64,
    /// Unix milliseconds (UTC) when it was appended.
    pub at: i64,
    /// The kind.
    pub kind: EventKind,
    /// The review the event belongs to, if any.
    pub review_id: Option<String>,
    /// The diff involved, if any.
    pub diff_id: Option<String>,
    /// The thread involved, if any.
    pub thread_id: Option<String>,
    /// The comment involved, if any.
    pub comment_id: Option<String>,
    /// Who caused it.
    pub actor: Actor,
    /// Kind-specific JSON; `Value::Null` when the row has none.
    pub payload: serde_json::Value,
}

/// Which events a reader wants. The default matches every event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventFilter {
    /// Only events whose `review_id` is in this list (`Some(vec![])` matches nothing).
    pub review_ids: Option<Vec<String>>,
    /// Only kinds with [`EventKind::agent_visible`].
    pub agent_visible_only: bool,
    /// Only these kinds (`Some(vec![])` matches nothing).
    pub kinds: Option<Vec<EventKind>>,
}

/// The current time in Unix milliseconds (the store's timestamp convention, §7.1).
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Appends `e` inside `tx` (the mutation's own transaction) and returns its `seq`.
pub fn append_event(tx: &Transaction, e: &NewEvent) -> Result<i64, StoreError> {
    let payload = if e.payload.is_null() {
        None
    } else {
        Some(serde_json::to_string(&e.payload)?)
    };
    tx.prepare_cached(
        "INSERT INTO events \
         (at, kind, review_id, diff_id, thread_id, comment_id, actor_kind, actor_name, session_id, payload) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?
    .execute(rusqlite::params![
        now_ms(),
        e.kind.as_str(),
        e.review_id,
        e.diff_id,
        e.thread_id,
        e.comment_id,
        e.actor.kind.as_str(),
        e.actor.name,
        e.actor.session_id,
        payload,
    ])?;
    Ok(tx.last_insert_rowid())
}

/// Up to `limit` events with `seq > after_seq` that match `filter`, in `seq` order.
/// Call it inside one read (e.g. [`Store::read`]) for a consistent snapshot.
pub fn events_since(
    conn: &Connection,
    after_seq: i64,
    filter: &EventFilter,
    limit: u32,
) -> Result<Vec<Event>, StoreError> {
    Ok(scan(conn, after_seq, None, filter, limit)?.events)
}

/// One page of a scan: the matching events, plus how many matching rows SQLite
/// returned (unknown kinds included) and the last such row's `seq`.
struct Page {
    events: Vec<Event>,
    rows: u32,
    last_seq: Option<i64>,
}

/// Reads matching rows with `after_seq < seq [<= up_to]`, at most `limit` of them.
fn scan(
    conn: &Connection,
    after_seq: i64,
    up_to: Option<i64>,
    filter: &EventFilter,
    limit: u32,
) -> Result<Page, StoreError> {
    let mut sql = String::from(
        "SELECT seq, at, kind, review_id, diff_id, thread_id, comment_id, \
         actor_kind, actor_name, session_id, payload FROM events WHERE seq > ?",
    );
    let mut args: Vec<SqlValue> = vec![after_seq.into()];
    if let Some(max) = up_to {
        sql.push_str(" AND seq <= ?");
        args.push(max.into());
    }
    if let Some(ids) = &filter.review_ids {
        push_in(&mut sql, &mut args, "review_id", ids.iter().cloned());
    }
    if let Some(kinds) = &filter.kinds {
        push_in(
            &mut sql,
            &mut args,
            "kind",
            kinds.iter().map(|k| k.as_str().to_owned()),
        );
    }
    if filter.agent_visible_only {
        let visible = EventKind::ALL.into_iter().filter(EventKind::agent_visible);
        push_in(
            &mut sql,
            &mut args,
            "kind",
            visible.map(|k| k.as_str().to_owned()),
        );
    }
    sql.push_str(" ORDER BY seq LIMIT ?");
    args.push(i64::from(limit).into());

    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query(params_from_iter(args))?;
    let mut page = Page {
        events: Vec::new(),
        rows: 0,
        last_seq: None,
    };
    while let Some(row) = rows.next()? {
        page.rows += 1;
        let seq: i64 = row.get(0)?;
        page.last_seq = Some(seq);
        if let Some(event) = event_from_row(row)? {
            page.events.push(event);
        }
    }
    Ok(page)
}

/// Appends ` AND <column> IN (?, …)` (or a never-true clause for an empty list).
fn push_in(
    sql: &mut String,
    args: &mut Vec<SqlValue>,
    column: &str,
    values: impl Iterator<Item = String>,
) {
    let start = args.len();
    args.extend(values.map(SqlValue::Text));
    let n = args.len() - start;
    if n == 0 {
        sql.push_str(" AND 0");
        return;
    }
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push_str(" IN (");
    sql.push_str(&vec!["?"; n].join(", "));
    sql.push(')');
}

/// Decodes a row; `None` (with a warning) for a kind this build does not know.
fn event_from_row(row: &Row<'_>) -> Result<Option<Event>, StoreError> {
    let seq: i64 = row.get(0)?;
    let kind_text: String = row.get(2)?;
    let Some(kind) = EventKind::parse(&kind_text) else {
        tracing::warn!(seq, kind = %kind_text, "skipping event of unknown kind");
        return Ok(None);
    };
    let actor_text: String = row.get(7)?;
    let actor_kind = ActorKind::parse(&actor_text).ok_or_else(|| {
        StoreError::Integrity(format!("event {seq} has actor_kind {actor_text:?}"))
    })?;
    let payload: Option<String> = row.get(10)?;
    let payload = match payload {
        Some(text) => serde_json::from_str(&text)?,
        None => serde_json::Value::Null,
    };
    Ok(Some(Event {
        seq,
        at: row.get(1)?,
        kind,
        review_id: row.get(3)?,
        diff_id: row.get(4)?,
        thread_id: row.get(5)?,
        comment_id: row.get(6)?,
        actor: Actor {
            kind: actor_kind,
            name: row.get(8)?,
            session_id: row.get(9)?,
        },
        payload,
    }))
}

/// A cross-process change feed over `events` (design §7.1 "Change notification").
///
/// Owns a dedicated connection that never writes (`PRAGMA query_only`) and never
/// holds a read transaction between polls, so it cannot starve checkpoints.
/// `data_version` only changes when *another* connection commits, which is every
/// writer, since this connection never writes. The connection is `Send`, so a feed
/// can live on a background thread.
pub struct EventFeed {
    conn: Connection,
    filter: EventFilter,
    cursor: i64,
    data_version: Option<i64>,
    scans: u64,
}

impl std::fmt::Debug for EventFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventFeed")
            .field("filter", &self.filter)
            .field("cursor", &self.cursor)
            .field("data_version", &self.data_version)
            .field("scans", &self.scans)
            .finish_non_exhaustive()
    }
}

impl EventFeed {
    /// Opens a feed that yields matching events with `seq > after_seq`. Creates and
    /// migrates the store first if needed (via [`Store::open`]), so it works against
    /// a missing database. The first [`EventFeed::poll`] always reads.
    ///
    /// `after_seq` is clamped to `0..=latest seq`: a cursor from the future (a
    /// stale or foreign `since`) would otherwise skip every event appended until
    /// the seq caught up with it.
    pub fn open(
        paths: &DataPaths,
        after_seq: i64,
        filter: EventFilter,
    ) -> Result<EventFeed, StoreError> {
        drop(Store::open(paths)?);
        // Read-write open + `query_only`, not SQLITE_OPEN_READ_ONLY: a read-only
        // connection cannot recreate the WAL index (`-shm`) after the last writer
        // closed and removed it.
        let conn = Connection::open_with_flags(
            &paths.db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        bootstrap_connection(&conn)?;
        conn.pragma_update(None, "query_only", "ON")?;
        let latest = latest_seq(&conn)?;
        Ok(EventFeed {
            conn,
            filter,
            cursor: after_seq.clamp(0, latest),
            data_version: None,
            scans: 0,
        })
    }

    /// New matching events since the last poll, in `seq` order. When no other
    /// connection has committed since the last poll, this only reads
    /// `PRAGMA data_version` and returns an empty list.
    pub fn poll(&mut self) -> Result<Vec<Event>, StoreError> {
        // Read data_version *before* the scan: a commit that lands in between is
        // either in this scan or bumps the version again for the next poll.
        let version: i64 = self
            .conn
            .prepare_cached("PRAGMA data_version")?
            .query_row([], |r| r.get(0))?;
        if self.data_version == Some(version) {
            return Ok(Vec::new());
        }

        let tx = self.conn.transaction()?;
        let latest = latest_seq(&tx)?;
        let mut cursor = self.cursor;
        let mut out = Vec::new();
        while cursor < latest {
            let page = scan(&tx, cursor, Some(latest), &self.filter, FEED_PAGE)?;
            out.extend(page.events);
            match page.last_seq {
                // A full page: more rows may match below `latest`.
                Some(last) if page.rows == FEED_PAGE => cursor = last,
                // Fewer rows than a page: nothing else matches up to `latest`.
                _ => cursor = latest,
            }
        }
        tx.commit()?;

        self.cursor = self.cursor.max(cursor);
        self.data_version = Some(version);
        self.scans += 1;
        Ok(out)
    }

    /// Moves the cursor to the latest event (never backwards) and returns it,
    /// so the next polls yield only events appended from now on. A reader
    /// that loads its state from the store when it starts (the app's store
    /// feed) calls this instead of reading the whole backlog.
    pub fn seek_to_latest(&mut self) -> Result<i64, StoreError> {
        self.cursor = self.cursor.max(latest_seq(&self.conn)?);
        Ok(self.cursor)
    }

    /// The highest `seq` in `events` (0 when empty), regardless of the filter.
    pub fn latest_seq(&self) -> Result<i64, StoreError> {
        latest_seq(&self.conn)
    }

    /// The feed's cursor: every matching event with `seq <= cursor` was returned.
    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    /// How many polls actually read `events` (diagnostics and tests).
    pub fn scans(&self) -> u64 {
        self.scans
    }

    /// The feed's own read-only connection, for follow-up reads after a poll. Do
    /// not leave a transaction open on it.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// The highest `seq` in `events` (0 when empty): the `latest_seq` agents pass
/// back as `since` (design §15.2).
pub fn latest_seq(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn
        .prepare_cached("SELECT COALESCE(MAX(seq), 0) FROM events")?
        .query_row([], |r| r.get(0))?)
}
