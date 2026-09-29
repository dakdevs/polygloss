//! Threads, comments, drafts, visibility, anchors and resolve (T1.13, design §8).
//!
//! Rules (design §7.3, §8.1–§8.4, ADR-0011, ADR-0020):
//!
//! - A human's new threads and replies are **drafts** (`published_at NULL`) until
//!   Submit review publishes them (`submit.rs`). Draft mutations append only the
//!   app-internal `draft.changed` event; `thread.created` / `comment.created` for
//!   human comments are written by the submission.
//! - Agent threads and replies are published at once, with their events.
//! - Agents never see draft comments, nor threads whose root comment is a draft
//!   or whose published comments are all deleted: such a thread is `NotFound` for
//!   them, to read and to reply to, resolve, edit or delete in.
//! - The human author is stored as `you` without a session, whatever name the
//!   caller passes.
//! - Agents create at most [`AGENT_THREAD_CAP`] threads per iteration (OQ-13);
//!   replies do not count. The count and the insert share one `BEGIN IMMEDIATE`
//!   transaction, so concurrent agents cannot overshoot.
//! - Anchors are validated against the stored file list of the thread's diff and
//!   the anchored blob (1-based inclusive lines, design §8.1), and capture
//!   `anchor_blob` and `anchor_snippet` (the anchored lines plus 3 lines of context
//!   on each side, joined with `\n`, invalid UTF-8 replaced).
//! - Resolve and unresolve take effect immediately (OQ-10), for humans too.
//! - Authors edit and delete only their own comments: same author kind, and for
//!   agents the same `author_name` (OQ-30). Deleting a draft removes it (a draft
//!   root takes its draft thread along); deleting a published comment soft-deletes
//!   it, and a deleted root that still has replies shows as a placeholder
//!   (design §8.2). A thread left without any undeleted comment (drafts
//!   included) is deleted, also when a draft reply was its last one.
//! - The root comment of a thread is its first comment by `(created_at, rowid)`.

use polygloss_diff::{FileChange, FileKind, FileStatus, ObjectFormat, Oid, Side};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ids::{DiffId, new_uuid};
use crate::objects::{BlobReader, is_binary};
use crate::review::models::{AWAITING_YOU_SQL, load_files};
use crate::review::open::{latest_iteration, review_exists};
use crate::review::{Core, CoreError};
use crate::store::StoreError;
use crate::store::events::{Actor, ActorKind, EventKind, NewEvent, append_event, now_ms};

/// Agent-created threads allowed per iteration (design §8.4, OQ-13).
pub const AGENT_THREAD_CAP: u32 = 50;

/// The stored name of the (single) human author and actor.
const HUMAN_NAME: &str = "you";

/// Lines of context on each side of the anchored lines in `anchor_snippet`.
const SNIPPET_CONTEXT: u32 = 3;

/// `threads.kind` (design §8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    /// A normal review thread (humans; agents may use it too).
    Comment,
    /// An agent explanation (collapsed by default). Agent-only.
    Note,
    /// An agent question that awaits the human. Agent-only.
    Question,
}

impl ThreadKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThreadKind::Comment => "comment",
            ThreadKind::Note => "note",
            ThreadKind::Question => "question",
        }
    }

    pub fn parse(s: &str) -> Option<ThreadKind> {
        [ThreadKind::Comment, ThreadKind::Note, ThreadKind::Question]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

/// `threads.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Open,
    Resolved,
}

impl ThreadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThreadStatus::Open => "open",
            ThreadStatus::Resolved => "resolved",
        }
    }

    pub fn parse(s: &str) -> Option<ThreadStatus> {
        match s {
            "open" => Some(ThreadStatus::Open),
            "resolved" => Some(ThreadStatus::Resolved),
            _ => None,
        }
    }
}

/// What a thread is about (design §8.1). Lines are 1-based and inclusive lines of
/// the blob on `side`; `start_line == line` for a single line.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case")]
pub enum Subject {
    Line {
        path: String,
        side: Side,
        start_line: u32,
        line: u32,
    },
    File {
        path: String,
    },
    Review,
}

impl Subject {
    /// The `threads.subject` value: `line`, `file` or `review`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Subject::Line { .. } => "line",
            Subject::File { .. } => "file",
            Subject::Review => "review",
        }
    }

    /// The anchored path (line and file subjects).
    pub fn path(&self) -> Option<&str> {
        match self {
            Subject::Line { path, .. } | Subject::File { path } => Some(path),
            Subject::Review => None,
        }
    }
}

/// `comments.author_kind` / `threads.created_by_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    Human,
    Agent,
}

impl AuthorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorKind::Human => "human",
            AuthorKind::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<AuthorKind> {
        match s {
            "human" => Some(AuthorKind::Human),
            "agent" => Some(AuthorKind::Agent),
            _ => None,
        }
    }
}

/// Who writes a comment: the human (`name` = `you`) or an agent (`name` = MCP
/// `clientInfo.name`, e.g. `claude-code`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Author {
    pub kind: AuthorKind,
    pub name: String,
    /// The agent session (canonical id), if any.
    pub session_id: Option<String>,
}

impl Author {
    /// The event actor for this author.
    pub fn actor(&self) -> Actor {
        Actor {
            kind: match self.kind {
                AuthorKind::Human => ActorKind::Human,
                AuthorKind::Agent => ActorKind::Agent,
            },
            name: Some(self.name.clone()),
            session_id: self.session_id.clone(),
        }
    }

    /// The author as stored: the human is always `you` without a session (the
    /// name a caller passes is ignored); agents are kept as given.
    fn normalized(&self) -> Author {
        match self.kind {
            AuthorKind::Human => Author {
                kind: AuthorKind::Human,
                name: HUMAN_NAME.into(),
                session_id: None,
            },
            AuthorKind::Agent => self.clone(),
        }
    }

    /// Whether this author owns a comment by `kind`/`name` (OQ-30): humans own
    /// every human comment; agents own comments with their `author_name`.
    fn owns(&self, kind: AuthorKind, name: &str) -> bool {
        self.kind == kind && (kind == AuthorKind::Human || self.name == name)
    }
}

/// A new thread with its root comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub review_id: String,
    /// The diff the anchor refers to (`origin_diff_id`); must be stored (pinned, or
    /// opened as commit/compare). Pin a live state first (`Core::pin_live_on_base`).
    pub diff_id: DiffId,
    pub subject: Subject,
    pub kind: ThreadKind,
    pub body_md: String,
    pub author: Author,
}

/// Who is reading: the human sees their drafts, agents never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Viewer {
    Human,
    Agent,
}

/// Which threads to list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThreadScope {
    /// The review's threads plus threads whose `origin_diff_id` is the review's
    /// current diff (its latest iteration), from other reviews or clones (§8.6).
    /// The human sees drafts of this review's threads only.
    Review(String),
    /// Threads whose `origin_diff_id` is this diff, plus the threads of every review
    /// with an iteration on it. The human sees all drafts.
    Diff(DiffId),
}

/// Thread list filters (MCP `list_threads`); the default matches everything.
/// Filters AND together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFilter {
    pub status: Option<ThreadStatus>,
    /// The thread's creator.
    pub author: Option<AuthorKind>,
    pub kind: Option<ThreadKind>,
    /// Threads anchored to this path (line and file subjects).
    pub path: Option<String>,
    /// Threads with an event after this seq (agent viewers: an agent-visible one).
    pub since_seq: Option<i64>,
}

/// The stored anchor of a thread: its subject (lines as created, never re-mapped;
/// carry-forward is T1.15) plus the blob and snippet captured for line subjects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadAnchor {
    pub subject: Subject,
    /// The blob the line numbers refer to (line subjects).
    pub anchor_blob: Option<Oid>,
    /// Lines `max(1, start_line - 3)..=min(n, line + 3)` of `anchor_blob`, joined
    /// with `\n` (line subjects).
    pub anchor_snippet: Option<String>,
}

/// Who resolved a thread and when.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResolvedBy {
    pub kind: AuthorKind,
    pub name: Option<String>,
    pub at: i64,
}

/// A comment as a viewer sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentView {
    pub id: String,
    pub thread_id: String,
    pub author: Author,
    /// Empty for a deleted comment's placeholder.
    pub body_md: String,
    /// Unpublished (only the human sees drafts).
    pub draft: bool,
    /// A "comment deleted" placeholder: a deleted root that still has replies.
    pub deleted: bool,
    pub published_at: Option<i64>,
    pub created_at: i64,
    /// Set when a published comment was edited.
    pub edited_at: Option<i64>,
    /// The submission that published a human comment.
    pub submission_id: Option<String>,
}

/// A thread with the comments the viewer may see, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadView {
    pub id: String,
    /// `None` only after the review was pruned elsewhere (never listed then).
    pub review_id: Option<String>,
    pub origin_diff_id: DiffId,
    pub origin_iteration_id: Option<i64>,
    pub kind: ThreadKind,
    pub anchor: ThreadAnchor,
    pub status: ThreadStatus,
    pub resolved_by: Option<ResolvedBy>,
    /// The creator (`session_id` from the root comment).
    pub created_by: Author,
    /// The root comment is a draft (only the human sees such threads).
    pub draft: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub comments: Vec<CommentView>,
}

impl ThreadView {
    /// "Awaiting you" (design §8.4): an open agent question without a published,
    /// undeleted human reply.
    pub fn awaiting_you(&self) -> bool {
        self.kind == ThreadKind::Question
            && self.status == ThreadStatus::Open
            && !self
                .comments
                .iter()
                .any(|c| c.author.kind == AuthorKind::Human && !c.draft && !c.deleted)
    }
}

/// What [`Core::delete_comment`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeletedComment {
    /// A deleted root with published replies left a "comment deleted" placeholder.
    pub placeholder: bool,
    /// The thread is gone: a draft root, or the last comment of the thread.
    pub thread_removed: bool,
}

/// The fields of a thread every mutation needs.
struct ThreadRow {
    review_id: Option<String>,
    origin_diff_id: String,
    status: String,
    root_draft: bool,
    /// What `load_thread` shows an agent: a published root and at least one
    /// published, undeleted comment.
    agent_visible: bool,
}

/// The fields of a comment edit and delete need.
struct CommentRow {
    thread_id: String,
    published: bool,
    is_root: bool,
    thread: ThreadRow,
}

/// A validated anchor, ready to store.
struct ResolvedAnchor {
    subject: Subject,
    blob: Option<Oid>,
    snippet: Option<String>,
}

impl Core {
    /// Creates a thread with its root comment and returns the thread id. Human
    /// threads are drafts; agent threads are published, capped per iteration
    /// (`CapExceeded`) and must be notes or questions or comments. The anchor is
    /// checked against the stored diff (`InvalidAnchor`); line anchors read the
    /// anchored blob through `blobs` (which must reach a pinned live state's
    /// objects, e.g. `BlobReader::open` after the pin).
    pub fn create_thread(&self, t: &NewThread, blobs: &BlobReader) -> Result<String, CoreError> {
        check_body(&t.body_md)?;
        if t.kind != ThreadKind::Comment && t.author.kind != AuthorKind::Agent {
            return Err(CoreError::InvalidRequest(format!(
                "only agents create {} threads",
                t.kind.as_str()
            )));
        }
        let (found, files) = self.store.read(|c| {
            if !review_exists(c, &t.review_id)? {
                return Ok((false, None));
            }
            Ok((true, load_files(c, &t.diff_id)?))
        })?;
        if !found {
            return Err(CoreError::not_found("review", &t.review_id));
        }
        let files = files.ok_or_else(|| CoreError::not_found("diff", t.diff_id.as_str()))?;
        let anchor = resolve_anchor(&t.subject, &files, blobs)?;

        let thread_id = new_uuid();
        let comment_id = new_uuid();
        let author = t.author.normalized();
        let agent = author.kind == AuthorKind::Agent;
        let now = now_ms();
        self.store.write(|tx| {
            // The review may have been pruned since the read.
            if !review_exists(tx, &t.review_id)? {
                return Ok(Err(CoreError::not_found("review", &t.review_id)));
            }
            let iteration: Option<i64> = tx
                .query_row(
                    "SELECT id FROM iterations WHERE review_id = ?1 AND diff_id = ?2 \
                     ORDER BY seq DESC LIMIT 1",
                    params![t.review_id, t.diff_id.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            if agent && agent_threads(tx, &t.review_id, &t.diff_id, iteration)? >= AGENT_THREAD_CAP
            {
                return Ok(Err(CoreError::CapExceeded {
                    cap: AGENT_THREAD_CAP,
                }));
            }
            let (path, side, start_line, line) = match &anchor.subject {
                Subject::Line {
                    path,
                    side,
                    start_line,
                    line,
                } => (
                    Some(path.as_str()),
                    Some(side_str(*side)),
                    Some(*start_line),
                    Some(*line),
                ),
                Subject::File { path } => (Some(path.as_str()), None, None, None),
                Subject::Review => (None, None, None, None),
            };
            tx.execute(
                "INSERT INTO threads (id, review_id, origin_diff_id, origin_iteration_id, subject, \
                   kind, path, side, start_line, line, anchor_blob, anchor_snippet, \
                   created_by_kind, created_by_name, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
                params![
                    thread_id,
                    t.review_id,
                    t.diff_id.as_str(),
                    iteration,
                    anchor.subject.as_str(),
                    t.kind.as_str(),
                    path,
                    side,
                    start_line,
                    line,
                    anchor.blob.as_ref().map(Oid::as_str),
                    anchor.snippet,
                    author.kind.as_str(),
                    author.name,
                    now
                ],
            )?;
            insert_comment(tx, &comment_id, &thread_id, &author, &t.body_md, now)?;
            touch_review(tx, &t.review_id, now)?;
            let event = if agent {
                thread_event(
                    EventKind::ThreadCreated,
                    Some(&t.review_id),
                    t.diff_id.as_str(),
                    &thread_id,
                    Some(&comment_id),
                    &author.actor(),
                    json!({ "kind": t.kind.as_str(), "subject": anchor.subject.as_str() }),
                )
            } else {
                draft_event(
                    Some(&t.review_id),
                    Some(&thread_id),
                    Some(&comment_id),
                    "created",
                    "thread",
                )
            };
            append_event(tx, &event)?;
            Ok(Ok(()))
        })??;
        Ok(thread_id)
    }

    /// Adds a reply and returns its id: a draft for the human, published for an
    /// agent (`comment.created`). Agents cannot reply to draft threads (`NotFound`).
    /// Replies to resolved or outdated threads are allowed and do not reopen them.
    pub fn reply(
        &self,
        thread_id: &str,
        body_md: &str,
        author: &Author,
    ) -> Result<String, CoreError> {
        check_body(body_md)?;
        let author = &author.normalized();
        let id = new_uuid();
        self.store.write(|tx| {
            let Some(th) = thread_row(tx, thread_id)? else {
                return Ok(Err(CoreError::not_found("thread", thread_id)));
            };
            if author.kind == AuthorKind::Agent && !th.agent_visible {
                return Ok(Err(CoreError::not_found("thread", thread_id)));
            }
            add_reply(tx, &th, thread_id, &id, body_md, author, now_ms())?;
            Ok(Ok(()))
        })??;
        Ok(id)
    }

    /// Edits the author's own comment (`Forbidden` otherwise). A draft just changes;
    /// a published comment gets `edited_at` and `comment.edited`.
    pub fn edit_comment(
        &self,
        comment_id: &str,
        body_md: &str,
        author: &Author,
    ) -> Result<(), CoreError> {
        check_body(body_md)?;
        self.store.write(|tx| {
            let c = match own_comment(tx, comment_id, author)? {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            let now = now_ms();
            if c.published {
                tx.execute(
                    "UPDATE comments SET body_md = ?2, edited_at = ?3 WHERE id = ?1",
                    params![comment_id, body_md, now],
                )?;
                touch_thread(tx, &c.thread_id, c.thread.review_id.as_deref(), now)?;
                append_event(
                    tx,
                    &thread_event(
                        EventKind::CommentEdited,
                        c.thread.review_id.as_deref(),
                        &c.thread.origin_diff_id,
                        &c.thread_id,
                        Some(comment_id),
                        &author.actor(),
                        serde_json::Value::Null,
                    ),
                )?;
            } else {
                tx.execute(
                    "UPDATE comments SET body_md = ?2 WHERE id = ?1",
                    params![comment_id, body_md],
                )?;
                if let Some(review) = &c.thread.review_id {
                    touch_review(tx, review, now)?;
                }
                append_event(
                    tx,
                    &draft_event(
                        c.thread.review_id.as_deref(),
                        Some(&c.thread_id),
                        Some(comment_id),
                        "edited",
                        "comment",
                    ),
                )?;
            }
            Ok(Ok(()))
        })?
    }

    /// Deletes the author's own comment (`Forbidden` otherwise). A draft is removed
    /// (a draft root with its whole draft thread). A published comment is
    /// soft-deleted with `comment.deleted`; a root that still has published
    /// replies leaves a placeholder. A thread left with no undeleted comment
    /// (drafts included) is deleted, whichever kind of comment went last.
    pub fn delete_comment(
        &self,
        comment_id: &str,
        author: &Author,
    ) -> Result<DeletedComment, CoreError> {
        self.store.write(|tx| {
            let c = match own_comment(tx, comment_id, author)? {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            let now = now_ms();
            let review = c.thread.review_id.as_deref();
            if !c.published {
                // A draft root takes its draft thread along; a draft reply does too
                // when it was the thread's last undeleted comment (e.g. under a
                // root its agent deleted), or the thread would linger unlisted.
                let thread_removed =
                    c.is_root || undeleted_others(tx, &c.thread_id, comment_id)?.0 == 0;
                if thread_removed {
                    tx.execute("DELETE FROM threads WHERE id = ?1", [&c.thread_id])?;
                } else {
                    tx.execute("DELETE FROM comments WHERE id = ?1", [comment_id])?;
                }
                if let Some(review) = review {
                    touch_review(tx, review, now)?;
                }
                append_event(
                    tx,
                    &draft_event(
                        review,
                        Some(&c.thread_id),
                        Some(comment_id),
                        "deleted",
                        "comment",
                    ),
                )?;
                return Ok(Ok(DeletedComment {
                    placeholder: false,
                    thread_removed,
                }));
            }
            let (others, published_others) = undeleted_others(tx, &c.thread_id, comment_id)?;
            let thread_removed = others == 0;
            if thread_removed {
                tx.execute("DELETE FROM threads WHERE id = ?1", [&c.thread_id])?;
                if let Some(review) = review {
                    touch_review(tx, review, now)?;
                }
            } else {
                tx.execute(
                    "UPDATE comments SET deleted_at = ?2 WHERE id = ?1",
                    params![comment_id, now],
                )?;
                touch_thread(tx, &c.thread_id, review, now)?;
            }
            append_event(
                tx,
                &thread_event(
                    EventKind::CommentDeleted,
                    review,
                    &c.thread.origin_diff_id,
                    &c.thread_id,
                    Some(comment_id),
                    &author.actor(),
                    serde_json::Value::Null,
                ),
            )?;
            Ok(Ok(DeletedComment {
                placeholder: c.is_root && !thread_removed && published_others > 0,
                thread_removed,
            }))
        })?
    }

    /// Resolves or unresolves a thread, immediately (OQ-10), recording who and when
    /// (`thread.resolved` / `thread.unresolved`). Setting the current status again
    /// changes nothing. `closing_reply` is added first as the actor's reply (a
    /// draft for the human, published for an agent). Draft threads cannot be
    /// resolved (`Conflict`; `NotFound` for agents); the system actor cannot
    /// resolve (`InvalidRequest`).
    pub fn set_resolved(
        &self,
        thread_id: &str,
        resolved: bool,
        actor: &Actor,
        closing_reply: Option<&str>,
    ) -> Result<(), CoreError> {
        let kind = match actor.kind {
            ActorKind::Human => AuthorKind::Human,
            ActorKind::Agent => AuthorKind::Agent,
            ActorKind::System => {
                return Err(CoreError::InvalidRequest(
                    "only humans and agents resolve threads".into(),
                ));
            }
        };
        if let Some(body) = closing_reply {
            check_body(body)?;
        }
        let author = Author {
            kind,
            name: actor.name.clone().unwrap_or_else(|| "agent".into()),
            session_id: actor.session_id.clone(),
        }
        .normalized();
        self.store.write(|tx| {
            let Some(th) = thread_row(tx, thread_id)? else {
                return Ok(Err(CoreError::not_found("thread", thread_id)));
            };
            if kind == AuthorKind::Agent && !th.agent_visible {
                return Ok(Err(CoreError::not_found("thread", thread_id)));
            }
            if th.root_draft {
                return Ok(Err(CoreError::Conflict(format!(
                    "thread {thread_id} is a draft; submit or delete it instead"
                ))));
            }
            let now = now_ms();
            if let Some(body) = closing_reply {
                add_reply(tx, &th, thread_id, &new_uuid(), body, &author, now)?;
            }
            let target = if resolved {
                ThreadStatus::Resolved
            } else {
                ThreadStatus::Open
            };
            if th.status == target.as_str() {
                return Ok(Ok(()));
            }
            if resolved {
                tx.execute(
                    "UPDATE threads SET status = 'resolved', resolved_by_kind = ?2, \
                       resolved_by_name = ?3, resolved_at = ?4, updated_at = ?4 WHERE id = ?1",
                    params![thread_id, kind.as_str(), author.name, now],
                )?;
            } else {
                tx.execute(
                    "UPDATE threads SET status = 'open', resolved_by_kind = NULL, \
                       resolved_by_name = NULL, resolved_at = NULL, updated_at = ?2 WHERE id = ?1",
                    params![thread_id, now],
                )?;
            }
            if let Some(review) = &th.review_id {
                touch_review(tx, review, now)?;
            }
            append_event(
                tx,
                &thread_event(
                    if resolved {
                        EventKind::ThreadResolved
                    } else {
                        EventKind::ThreadUnresolved
                    },
                    th.review_id.as_deref(),
                    &th.origin_diff_id,
                    thread_id,
                    None,
                    &author.actor(),
                    serde_json::Value::Null,
                ),
            )?;
            Ok(Ok(()))
        })?
    }

    /// The threads in `scope` the viewer may see, oldest first (`NotFound` for an
    /// unknown review or diff).
    pub fn threads(
        &self,
        scope: ThreadScope,
        viewer: Viewer,
        filter: &ThreadFilter,
    ) -> Result<Vec<ThreadView>, CoreError> {
        self.store.read(|c| {
            let mut args: Vec<SqlValue> = Vec::new();
            let (scope_sql, own_review) = match &scope {
                ThreadScope::Review(review) => {
                    if !review_exists(c, review)? {
                        return Ok(Err(CoreError::not_found("review", review)));
                    }
                    let current = latest_iteration(c, review)?.map(|it| it.diff_id);
                    args.push(SqlValue::Text(review.clone()));
                    args.push(match current {
                        Some(d) => SqlValue::Text(d.as_str().to_owned()),
                        None => SqlValue::Null,
                    });
                    (
                        "(t.review_id = ?1 OR t.origin_diff_id = ?2)",
                        Some(review.as_str()),
                    )
                }
                ThreadScope::Diff(diff) => {
                    let known = c
                        .query_row("SELECT 1 FROM diffs WHERE id = ?1", [diff.as_str()], |_| {
                            Ok(())
                        })
                        .optional()?
                        .is_some();
                    if !known {
                        return Ok(Err(CoreError::not_found("diff", diff.as_str())));
                    }
                    args.push(SqlValue::Text(diff.as_str().to_owned()));
                    (
                        "(t.origin_diff_id = ?1 OR t.review_id IN \
                           (SELECT review_id FROM iterations WHERE diff_id = ?1))",
                        None,
                    )
                }
            };
            let mut sql = format!("SELECT t.id FROM threads t WHERE {scope_sql}");
            let mut push = |cond: &str, value: SqlValue| {
                args.push(value);
                sql.push_str(&cond.replace("?N", &format!("?{}", args.len())));
            };
            if let Some(status) = filter.status {
                push(" AND t.status = ?N", status.as_str().to_owned().into());
            }
            if let Some(author) = filter.author {
                push(
                    " AND t.created_by_kind = ?N",
                    author.as_str().to_owned().into(),
                );
            }
            if let Some(kind) = filter.kind {
                push(" AND t.kind = ?N", kind.as_str().to_owned().into());
            }
            if let Some(path) = &filter.path {
                push(" AND t.path = ?N", path.clone().into());
            }
            if let Some(since) = filter.since_seq {
                let hidden = if viewer == Viewer::Agent {
                    " AND e.kind NOT IN ('viewed.changed', 'draft.changed')"
                } else {
                    ""
                };
                push(
                    &format!(
                        " AND EXISTS (SELECT 1 FROM events e WHERE e.thread_id = t.id \
                           AND e.seq > ?N{hidden})"
                    ),
                    since.into(),
                );
            }
            sql.push_str(" ORDER BY t.created_at, t.rowid");
            let ids = {
                let mut stmt = c.prepare(&sql)?;
                stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let drafts = |review: Option<&str>| own_review.is_none() || review == own_review;
                if let Some(view) = load_thread(c, &id, viewer, drafts)? {
                    out.push(view);
                }
            }
            Ok(Ok(out))
        })?
    }

    /// One thread as the viewer sees it (`NotFound` when it does not exist for
    /// them, e.g. a draft thread for an agent).
    pub fn thread(&self, thread_id: &str, viewer: Viewer) -> Result<ThreadView, CoreError> {
        self.store
            .read(|c| load_thread(c, thread_id, viewer, |_| true))?
            .ok_or_else(|| CoreError::not_found("thread", thread_id))
    }

    /// The review's unpublished comments (the Submit button's count).
    pub fn drafts_count(&self, review_id: &str) -> Result<u32, CoreError> {
        let n = self.store.read(|c| {
            if !review_exists(c, review_id)? {
                return Ok(None);
            }
            Ok(Some(c.query_row(
                "SELECT count(*) FROM comments c JOIN threads t ON t.id = c.thread_id \
                 WHERE t.review_id = ?1 AND c.published_at IS NULL AND c.deleted_at IS NULL",
                [review_id],
                |r| r.get::<_, u32>(0),
            )?))
        })?;
        n.ok_or_else(|| CoreError::not_found("review", review_id))
    }

    /// Whether the review awaits the human (design §17): re-review requested, or an
    /// open agent question without a published, undeleted human reply. The same
    /// predicate as review summaries and stale pruning.
    pub fn review_awaiting_you(&self, review_id: &str) -> Result<bool, CoreError> {
        self.store
            .read(|c| {
                Ok(c.query_row(
                    &format!("SELECT {AWAITING_YOU_SQL} FROM reviews r WHERE r.id = ?1"),
                    [review_id],
                    |r| r.get::<_, bool>(0),
                )
                .optional()?)
            })?
            .ok_or_else(|| CoreError::not_found("review", review_id))
    }
}

fn check_body(body_md: &str) -> Result<(), CoreError> {
    if body_md.trim().is_empty() {
        return Err(CoreError::InvalidRequest(
            "the comment body is empty".into(),
        ));
    }
    Ok(())
}

pub(crate) fn side_str(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}

fn parse_side(s: &str) -> Option<Side> {
    match s {
        "old" => Some(Side::Old),
        "new" => Some(Side::New),
        _ => None,
    }
}

/// Agent threads already counted against the cap of `iteration` (or, for a diff
/// that is no iteration of the review, of the review and diff).
fn agent_threads(
    tx: &Transaction,
    review_id: &str,
    diff_id: &DiffId,
    iteration: Option<i64>,
) -> Result<u32, StoreError> {
    Ok(match iteration {
        Some(it) => tx.query_row(
            "SELECT count(*) FROM threads WHERE created_by_kind = 'agent' \
             AND origin_iteration_id = ?1",
            [it],
            |r| r.get(0),
        )?,
        None => tx.query_row(
            "SELECT count(*) FROM threads WHERE created_by_kind = 'agent' \
             AND review_id = ?1 AND origin_diff_id = ?2 AND origin_iteration_id IS NULL",
            params![review_id, diff_id.as_str()],
            |r| r.get(0),
        )?,
    })
}

/// Validates `subject` against the diff's files and captures the anchor blob and
/// snippet (design §8.1).
fn resolve_anchor(
    subject: &Subject,
    files: &[FileChange],
    blobs: &BlobReader,
) -> Result<ResolvedAnchor, CoreError> {
    let invalid = |msg: String| Err(CoreError::InvalidAnchor(msg));
    match subject {
        Subject::Review => Ok(ResolvedAnchor {
            subject: Subject::Review,
            blob: None,
            snippet: None,
        }),
        Subject::File { path } => match find_file(files, path) {
            Some(f) => Ok(ResolvedAnchor {
                subject: Subject::File {
                    path: f.display_path().to_owned(),
                },
                blob: None,
                snippet: None,
            }),
            None => invalid(format!("{path} is not part of this diff")),
        },
        Subject::Line {
            path,
            side,
            start_line,
            line,
        } => {
            let Some(f) = find_file(files, path) else {
                return invalid(format!("{path} is not part of this diff"));
            };
            if *start_line == 0 || *start_line > *line {
                return invalid(format!(
                    "lines are 1-based with start_line <= line (got {start_line}..{line})"
                ));
            }
            let blob = match (side, f.status) {
                (Side::Old, FileStatus::Added) => {
                    return invalid(format!("{path} is added: it has no old side"));
                }
                (Side::New, FileStatus::Deleted) => {
                    return invalid(format!("{path} is deleted: it has no new side"));
                }
                (Side::Old, _) => &f.old_blob,
                (Side::New, _) => &f.new_blob,
            };
            if matches!(f.kind, FileKind::Binary | FileKind::Submodule) {
                return invalid(format!("{path} has no lines (binary or submodule)"));
            }
            let bytes = blobs.read(blob)?;
            if is_binary(&bytes) {
                return invalid(format!("{path} has no lines (binary)"));
            }
            let lines = split_lines(&bytes);
            let n = u32::try_from(lines.len()).unwrap_or(u32::MAX);
            if *line > n {
                return invalid(format!(
                    "line {line} is beyond the end of {path} ({n} lines on the {} side)",
                    side_str(*side)
                ));
            }
            let from = start_line.saturating_sub(SNIPPET_CONTEXT).max(1);
            let to = line.saturating_add(SNIPPET_CONTEXT).min(n);
            let snippet = snippet(&lines, from, to);
            Ok(ResolvedAnchor {
                subject: Subject::Line {
                    path: f.display_path().to_owned(),
                    side: *side,
                    start_line: *start_line,
                    line: *line,
                },
                blob: Some(blob.clone()),
                snippet: Some(snippet),
            })
        }
    }
}

/// The file change for `path`: by new path first, else by old path (a renamed or
/// deleted file's old name).
fn find_file<'a>(files: &'a [FileChange], path: &str) -> Option<&'a FileChange> {
    let is = |p: &Option<polygloss_diff::GitPath>| p.as_ref().is_some_and(|p| p.text == path);
    files
        .iter()
        .find(|f| is(&f.new_path))
        .or_else(|| files.iter().find(|f| is(&f.old_path)))
}

/// The blob's lines without their `\n` (git's line count: a final line without a
/// newline counts; an empty blob has none).
fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if bytes.is_empty() {
        return Vec::new();
    }
    body.split(|b| *b == b'\n').collect()
}

/// Lines `from..=to` (1-based, in range) joined with `\n`, each without a final
/// `\r` (CRLF files), invalid UTF-8 replaced.
fn snippet(lines: &[&[u8]], from: u32, to: u32) -> String {
    lines[(from - 1) as usize..to as usize]
        .iter()
        .map(|l| String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Inserts a comment (published at once for agents). A `session_id` that is not
/// in `sessions` (the MCP server upserts its session on initialize, T1.14/T4.4) is
/// stored as `NULL` rather than failing the foreign key: the session is metadata.
fn insert_comment(
    tx: &Transaction,
    id: &str,
    thread_id: &str,
    author: &Author,
    body_md: &str,
    now: i64,
) -> Result<(), StoreError> {
    let published = (author.kind == AuthorKind::Agent).then_some(now);
    tx.execute(
        "INSERT INTO comments (id, thread_id, author_kind, author_name, session_id, body_md, \
           published_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, (SELECT id FROM sessions WHERE id = ?5), ?6, ?7, ?8)",
        params![
            id,
            thread_id,
            author.kind.as_str(),
            author.name,
            author.session_id,
            body_md,
            published,
            now
        ],
    )?;
    Ok(())
}

/// Inserts a reply with its event (published for agents, a draft for the human).
fn add_reply(
    tx: &Transaction,
    th: &ThreadRow,
    thread_id: &str,
    id: &str,
    body_md: &str,
    author: &Author,
    now: i64,
) -> Result<(), StoreError> {
    insert_comment(tx, id, thread_id, author, body_md, now)?;
    let review = th.review_id.as_deref();
    let event = if author.kind == AuthorKind::Agent {
        touch_thread(tx, thread_id, review, now)?;
        thread_event(
            EventKind::CommentCreated,
            review,
            &th.origin_diff_id,
            thread_id,
            Some(id),
            &author.actor(),
            serde_json::Value::Null,
        )
    } else {
        if let Some(review) = review {
            touch_review(tx, review, now)?;
        }
        draft_event(review, Some(thread_id), Some(id), "created", "comment")
    };
    append_event(tx, &event)?;
    Ok(())
}

/// `(all, published)` counts of the thread's undeleted comments other than
/// `comment_id` (drafts included in `all`).
fn undeleted_others(
    tx: &Transaction,
    thread_id: &str,
    comment_id: &str,
) -> Result<(i64, i64), StoreError> {
    Ok(tx.query_row(
        "SELECT count(*), count(published_at) FROM comments \
         WHERE thread_id = ?1 AND id <> ?2 AND deleted_at IS NULL",
        params![thread_id, comment_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn touch_review(tx: &Transaction, review_id: &str, now: i64) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE reviews SET updated_at = ?2 WHERE id = ?1",
        params![review_id, now],
    )?;
    Ok(())
}

/// Bumps the thread's and its review's `updated_at` (published changes only, so
/// "updated since" never reveals drafts).
fn touch_thread(
    tx: &Transaction,
    thread_id: &str,
    review_id: Option<&str>,
    now: i64,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE threads SET updated_at = ?2 WHERE id = ?1",
        params![thread_id, now],
    )?;
    if let Some(review) = review_id {
        touch_review(tx, review, now)?;
    }
    Ok(())
}

pub(crate) fn thread_event(
    kind: EventKind,
    review_id: Option<&str>,
    diff_id: &str,
    thread_id: &str,
    comment_id: Option<&str>,
    actor: &Actor,
    payload: serde_json::Value,
) -> NewEvent {
    NewEvent {
        kind,
        review_id: review_id.map(str::to_owned),
        diff_id: Some(diff_id.to_owned()),
        thread_id: Some(thread_id.to_owned()),
        comment_id: comment_id.map(str::to_owned),
        actor: actor.clone(),
        payload,
    }
}

/// The app-internal `draft.changed` event (never seen by agents, §7.3). Payload
/// `{op: created|edited|deleted, target: thread|comment|submit_dialog}`.
pub(crate) fn draft_event(
    review_id: Option<&str>,
    thread_id: Option<&str>,
    comment_id: Option<&str>,
    op: &str,
    target: &str,
) -> NewEvent {
    NewEvent {
        kind: EventKind::DraftChanged,
        review_id: review_id.map(str::to_owned),
        diff_id: None,
        thread_id: thread_id.map(str::to_owned),
        comment_id: comment_id.map(str::to_owned),
        actor: Actor {
            kind: ActorKind::Human,
            name: Some(HUMAN_NAME.into()),
            session_id: None,
        },
        payload: json!({ "op": op, "target": target }),
    }
}

/// SQL for the id of a thread's root comment (`?1` = thread id).
const ROOT_COMMENT_SQL: &str =
    "(SELECT r.id FROM comments r WHERE r.thread_id = ?1 ORDER BY r.created_at, r.rowid LIMIT 1)";

fn thread_row(conn: &Connection, thread_id: &str) -> Result<Option<ThreadRow>, StoreError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT review_id, origin_diff_id, status, \
                   (SELECT published_at IS NULL FROM comments WHERE id = {ROOT_COMMENT_SQL}), \
                   EXISTS (SELECT 1 FROM comments WHERE thread_id = ?1 \
                     AND published_at IS NOT NULL AND deleted_at IS NULL) \
                 FROM threads WHERE id = ?1"
            ),
            [thread_id],
            |r| {
                let root_draft = r.get::<_, Option<bool>>(3)?.unwrap_or(false);
                Ok(ThreadRow {
                    review_id: r.get(0)?,
                    origin_diff_id: r.get(1)?,
                    status: r.get(2)?,
                    root_draft,
                    agent_visible: !root_draft && r.get::<_, bool>(4)?,
                })
            },
        )
        .optional()?)
}

/// The comment if `author` may change it: `NotFound` when missing, deleted, or a
/// draft the author (an agent) cannot see; `Forbidden` when someone else's.
fn own_comment(
    tx: &Transaction,
    comment_id: &str,
    author: &Author,
) -> Result<Result<CommentRow, CoreError>, StoreError> {
    let row = tx
        .query_row(
            &format!(
                "SELECT c.thread_id, c.author_kind, c.author_name, c.published_at IS NOT NULL, \
                   c.deleted_at IS NOT NULL, c.id = {} FROM comments c WHERE c.id = ?1",
                ROOT_COMMENT_SQL.replace("?1", "c.thread_id")
            ),
            [comment_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, bool>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            },
        )
        .optional()?;
    let not_found = || Ok(Err(CoreError::not_found("comment", comment_id)));
    let Some((thread_id, kind, author_name, published, deleted, is_root)) = row else {
        return not_found();
    };
    let author_kind = AuthorKind::parse(&kind)
        .ok_or_else(|| StoreError::Integrity(format!("comment author_kind {kind:?}")))?;
    if deleted || (!published && author.kind == AuthorKind::Agent) {
        return not_found();
    }
    let Some(thread) = thread_row(tx, &thread_id)? else {
        return not_found();
    };
    if author.kind == AuthorKind::Agent && !thread.agent_visible {
        return not_found();
    }
    if !author.owns(author_kind, &author_name) {
        return Ok(Err(CoreError::Forbidden(format!(
            "comment {comment_id} belongs to {} {author_name}",
            author_kind.as_str()
        ))));
    }
    Ok(Ok(CommentRow {
        thread_id,
        published,
        is_root,
        thread,
    }))
}

/// Loads a thread as `viewer` sees it; `drafts(review_id)` says whether the human
/// sees this thread's drafts. `None` when the thread does not exist for the viewer.
fn load_thread(
    conn: &Connection,
    thread_id: &str,
    viewer: Viewer,
    drafts: impl Fn(Option<&str>) -> bool,
) -> Result<Option<ThreadView>, StoreError> {
    let bad =
        |what: &str, v: &str| StoreError::Integrity(format!("thread {thread_id}: {what} {v:?}"));
    let row = conn
        .query_row(
            "SELECT t.review_id, t.origin_diff_id, t.origin_iteration_id, t.subject, t.kind, \
               t.path, t.side, t.start_line, t.line, t.anchor_blob, t.anchor_snippet, t.status, \
               t.resolved_by_kind, t.resolved_by_name, t.resolved_at, t.created_by_kind, \
               t.created_by_name, t.created_at, t.updated_at, d.object_format \
             FROM threads t JOIN diffs d ON d.id = t.origin_diff_id WHERE t.id = ?1",
            [thread_id],
            |r| {
                Ok((
                    (
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ),
                    (
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<u32>>(7)?,
                        r.get::<_, Option<u32>>(8)?,
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<String>>(10)?,
                    ),
                    (
                        r.get::<_, String>(11)?,
                        r.get::<_, Option<String>>(12)?,
                        r.get::<_, Option<String>>(13)?,
                        r.get::<_, Option<i64>>(14)?,
                    ),
                    (
                        r.get::<_, String>(15)?,
                        r.get::<_, String>(16)?,
                        r.get::<_, i64>(17)?,
                        r.get::<_, i64>(18)?,
                        r.get::<_, String>(19)?,
                    ),
                ))
            },
        )
        .optional()?;
    let Some((
        (review_id, origin_diff, origin_iteration_id, subject, kind),
        (path, side, start_line, line, anchor_blob, anchor_snippet),
        (status, resolved_kind, resolved_name, resolved_at),
        (created_kind, created_name, created_at, updated_at, fmt),
    )) = row
    else {
        return Ok(None);
    };
    let fmt = ObjectFormat::from_name(&fmt).ok_or_else(|| bad("object format", &fmt))?;

    let mut stmt = conn.prepare_cached(
        "SELECT id, author_kind, author_name, session_id, body_md, submission_id, published_at, \
           created_at, edited_at, deleted_at FROM comments WHERE thread_id = ?1 \
         ORDER BY created_at, rowid",
    )?;
    let rows = stmt
        .query_map([thread_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, Option<i64>>(8)?,
                r.get::<_, Option<i64>>(9)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let Some(root) = rows.first() else {
        return Ok(None);
    };
    let root_draft = root.6.is_none();
    let root_session = root.3.clone();
    let show_drafts = viewer == Viewer::Human && drafts(review_id.as_deref());
    if root_draft && !show_drafts {
        return Ok(None);
    }
    let mut all = Vec::with_capacity(rows.len());
    for (id, a_kind, a_name, session, body, submission, published, c_at, edited, deleted) in rows {
        let draft = published.is_none();
        if draft && !show_drafts {
            continue;
        }
        all.push(CommentView {
            id,
            thread_id: thread_id.to_owned(),
            author: Author {
                kind: AuthorKind::parse(&a_kind).ok_or_else(|| bad("author_kind", &a_kind))?,
                name: a_name,
                session_id: session,
            },
            body_md: body,
            draft,
            deleted: deleted.is_some(),
            published_at: published,
            created_at: c_at,
            edited_at: edited,
            submission_id: submission,
        });
    }
    let live = all.iter().filter(|c| !c.deleted).count();
    if live == 0 {
        return Ok(None);
    }
    // Only a deleted root with visible replies stays, as a placeholder.
    let root_id = all.first().map(|c| c.id.clone());
    let comments = all
        .into_iter()
        .filter_map(|mut c| {
            if !c.deleted {
                return Some(c);
            }
            if Some(&c.id) == root_id.as_ref() {
                c.body_md.clear();
                return Some(c);
            }
            None
        })
        .collect();

    let subject = match subject.as_str() {
        "review" => Subject::Review,
        "file" => Subject::File {
            path: path.ok_or_else(|| bad("path", ""))?,
        },
        "line" => {
            let side = side.unwrap_or_default();
            Subject::Line {
                path: path.ok_or_else(|| bad("path", ""))?,
                side: parse_side(&side).ok_or_else(|| bad("side", &side))?,
                start_line: start_line.ok_or_else(|| bad("start_line", ""))?,
                line: line.ok_or_else(|| bad("line", ""))?,
            }
        }
        other => return Err(bad("subject", other)),
    };
    let anchor_blob = anchor_blob
        .map(|b| Oid::parse(&b, fmt).map_err(|_| bad("anchor_blob", &b)))
        .transpose()?;
    let resolved_by = match (resolved_kind, resolved_at) {
        (Some(k), Some(at)) => Some(ResolvedBy {
            kind: AuthorKind::parse(&k).ok_or_else(|| bad("resolved_by_kind", &k))?,
            name: resolved_name,
            at,
        }),
        _ => None,
    };
    Ok(Some(ThreadView {
        id: thread_id.to_owned(),
        review_id,
        origin_diff_id: DiffId::parse(&origin_diff)
            .map_err(|_| bad("origin_diff_id", &origin_diff))?,
        origin_iteration_id,
        kind: ThreadKind::parse(&kind).ok_or_else(|| bad("kind", &kind))?,
        anchor: ThreadAnchor {
            subject,
            anchor_blob,
            anchor_snippet,
        },
        status: ThreadStatus::parse(&status).ok_or_else(|| bad("status", &status))?,
        resolved_by,
        created_by: Author {
            kind: AuthorKind::parse(&created_kind)
                .ok_or_else(|| bad("created_by_kind", &created_kind))?,
            name: created_name,
            session_id: root_session,
        },
        draft: root_draft,
        created_at,
        updated_at,
        comments,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_roundtrip_their_column_text() {
        for k in [ThreadKind::Comment, ThreadKind::Note, ThreadKind::Question] {
            assert_eq!(ThreadKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
        for s in [ThreadStatus::Open, ThreadStatus::Resolved] {
            assert_eq!(ThreadStatus::parse(s.as_str()), Some(s));
        }
        for a in [AuthorKind::Human, AuthorKind::Agent] {
            assert_eq!(AuthorKind::parse(a.as_str()), Some(a));
        }
        for s in [Side::Old, Side::New] {
            assert_eq!(parse_side(side_str(s)), Some(s));
        }
        assert_eq!(ThreadKind::parse("x"), None);
        assert_eq!(
            serde_json::to_value(Subject::File { path: "a".into() }).unwrap(),
            json!({ "subject": "file", "path": "a" })
        );
    }

    #[test]
    fn split_lines_counts_like_git() {
        assert_eq!(split_lines(b""), Vec::<&[u8]>::new());
        assert_eq!(split_lines(b"\n"), [&b""[..]]);
        assert_eq!(split_lines(b"a"), [&b"a"[..]]);
        assert_eq!(split_lines(b"a\nb"), [&b"a"[..], b"b"]);
        assert_eq!(split_lines(b"a\nb\n"), [&b"a"[..], b"b"]);
        assert_eq!(split_lines(b"a\n\n"), [&b"a"[..], b""]);
    }

    #[test]
    fn snippet_joins_lines_without_carriage_returns() {
        let lines = split_lines(b"a\r\nb\r\n\xffc\r\nd");
        assert_eq!(snippet(&lines, 1, 4), "a\nb\n\u{fffd}c\nd");
        assert_eq!(snippet(&lines, 2, 3), "b\n\u{fffd}c");
        // Only a final `\r` is a line ending; one inside the line stays.
        assert_eq!(snippet(&split_lines(b"x\ry\r\r\n"), 1, 1), "x\ry\r");
    }

    #[test]
    fn human_authors_are_normalized_to_you() {
        let bob = Author {
            kind: AuthorKind::Human,
            name: "bob".into(),
            session_id: Some("s".into()),
        };
        let n = bob.normalized();
        assert_eq!(n.name, "you");
        assert_eq!(n.session_id, None);
        let agent = Author {
            kind: AuthorKind::Agent,
            name: "claude-code".into(),
            session_id: Some("s".into()),
        };
        assert_eq!(agent.normalized(), agent);
    }

    #[test]
    fn authors_own_by_kind_and_agent_name() {
        let agent = Author {
            kind: AuthorKind::Agent,
            name: "claude-code".into(),
            session_id: None,
        };
        assert!(agent.owns(AuthorKind::Agent, "claude-code"));
        assert!(!agent.owns(AuthorKind::Agent, "other"));
        assert!(!agent.owns(AuthorKind::Human, "claude-code"));
        let human = Author {
            kind: AuthorKind::Human,
            name: "you".into(),
            session_id: None,
        };
        assert!(human.owns(AuthorKind::Human, "someone"));
        assert!(!human.owns(AuthorKind::Agent, "you"));
    }
}
