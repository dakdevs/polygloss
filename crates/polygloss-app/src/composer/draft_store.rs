//! What a composer writes and where (design §8.1–§8.3, OQ-31): composer
//! keys, the thread subject they anchor, and the save itself through
//! `Core` (a new thread, a reply or an edit; all drafts for the human).
//!
//! Keys name a composer's target and survive a relaunch: unsaved text is
//! autosaved in `ViewState.composer` under [`ComposerKey::to_string`]
//! (design §8.3 "Unsaved composer text is autosaved in `view_state`").
//!
//! | Key                                 | Composer                      |
//! | ----------------------------------- | ----------------------------- |
//! | `line:<side>:<start>:<line>:<path>` | a new line or range thread    |
//! | `file:<path>`                       | a new file thread             |
//! | `review`                            | a new review-level thread     |
//! | `reply:<thread id>`                 | a reply                       |
//! | `edit:<comment id>`                 | editing one's own comment     |
//!
//! Line numbers in keys and subjects are 1-based, as stored (design §8.1);
//! the viewport's cursor is 0-based (T3.8), so [`ComposerKey::line`] adds 1.
//!
//! Saving a new thread on a live diff pins the state shown first
//! (`Core::pin_live_on_base(…, PinnedBy::Comment, …)`, design §5.2): a
//! thread needs a stored diff, and it is the displayed base that is pinned,
//! never a re-resolved one (T1.13).

use std::fmt;
use std::path::PathBuf;

use polygloss_core::git::{LiveState, RepoInfo, ResolvedSide};
use polygloss_core::ids::DiffId;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, IterationInfo, NewThread, PinnedBy, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;
use polygloss_viewport::BlockId;

/// What a composer writes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ComposerKey {
    /// A new thread on lines `start_line..=line` (1-based) of `side`.
    Line {
        path: String,
        side: Side,
        start_line: u32,
        line: u32,
    },
    /// A new thread on a file (under its header).
    File { path: String },
    /// A new review-level thread (in the threads panel).
    Review,
    /// A reply to a thread.
    Reply { thread_id: String },
    /// An edit of one's own comment.
    Edit { comment_id: String },
}

impl ComposerKey {
    /// The key of a line composer from the viewport's 0-based lines.
    pub fn line(path: &str, side: Side, start_line: u32, line: u32) -> ComposerKey {
        let (a, b) = (start_line.min(line), start_line.max(line));
        ComposerKey::Line {
            path: path.to_owned(),
            side,
            start_line: a + 1,
            line: b + 1,
        }
    }

    /// Parses [`ComposerKey::to_string`]'s form; `None` for anything else.
    pub fn parse(s: &str) -> Option<ComposerKey> {
        if s == "review" {
            return Some(ComposerKey::Review);
        }
        let (kind, rest) = s.split_once(':')?;
        match kind {
            "line" => {
                let mut parts = rest.splitn(4, ':');
                let side = match parts.next()? {
                    "old" => Side::Old,
                    "new" => Side::New,
                    _ => return None,
                };
                let start_line: u32 = parts.next()?.parse().ok()?;
                let line: u32 = parts.next()?.parse().ok()?;
                let path = parts.next()?;
                (start_line >= 1 && start_line <= line && !path.is_empty()).then(|| {
                    ComposerKey::Line {
                        path: path.to_owned(),
                        side,
                        start_line,
                        line,
                    }
                })
            }
            "file" if !rest.is_empty() => Some(ComposerKey::File {
                path: rest.to_owned(),
            }),
            "reply" if !rest.is_empty() => Some(ComposerKey::Reply {
                thread_id: rest.to_owned(),
            }),
            "edit" if !rest.is_empty() => Some(ComposerKey::Edit {
                comment_id: rest.to_owned(),
            }),
            _ => None,
        }
    }

    /// The subject of the thread a new-thread composer creates (`None` for
    /// replies and edits).
    pub fn subject(&self) -> Option<Subject> {
        match self {
            ComposerKey::Line {
                path,
                side,
                start_line,
                line,
            } => Some(Subject::Line {
                path: path.clone(),
                side: *side,
                start_line: *start_line,
                line: *line,
            }),
            ComposerKey::File { path } => Some(Subject::File { path: path.clone() }),
            ComposerKey::Review => Some(Subject::Review),
            ComposerKey::Reply { .. } | ComposerKey::Edit { .. } => None,
        }
    }

    /// Whether saving creates a thread (and so may pin a live diff).
    pub fn is_new_thread(&self) -> bool {
        self.subject().is_some()
    }

    /// The path of a line or file composer.
    pub fn path(&self) -> Option<&str> {
        match self {
            ComposerKey::Line { path, .. } | ComposerKey::File { path } => Some(path),
            _ => None,
        }
    }

    /// The viewport block of a line or file composer: FNV-1a of
    /// `"composer:" + key` (thread blocks hash `"thread:"`, so the two never
    /// collide by construction of their prefixes).
    pub fn block_id(&self) -> BlockId {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in b"composer:".iter().chain(self.to_string().as_bytes()) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        BlockId(h)
    }

    /// What the composer's header says: "Comment on line 12", "Comment on
    /// lines 12–14" (old-side lines read `L12`, GitHub's left), "Comment on
    /// file", "Comment on review", "Reply", "Edit comment".
    pub fn title(&self) -> String {
        match self {
            ComposerKey::Line {
                side,
                start_line,
                line,
                ..
            } => {
                let l = if *side == Side::Old { "L" } else { "" };
                if start_line < line {
                    format!("Comment on lines {l}{start_line}–{l}{line}")
                } else {
                    format!("Comment on line {l}{line}")
                }
            }
            ComposerKey::File { .. } => "Comment on file".to_owned(),
            ComposerKey::Review => "Comment on review".to_owned(),
            ComposerKey::Reply { .. } => "Reply".to_owned(),
            ComposerKey::Edit { .. } => "Edit comment".to_owned(),
        }
    }

    /// The save button's label.
    pub fn save_label(&self) -> &'static str {
        match self {
            ComposerKey::Edit { .. } => "Update comment",
            ComposerKey::Reply { .. } => "Save reply",
            _ => "Save draft",
        }
    }
}

impl fmt::Display for ComposerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComposerKey::Line {
                path,
                side,
                start_line,
                line,
            } => {
                let side = match side {
                    Side::Old => "old",
                    Side::New => "new",
                };
                write!(f, "line:{side}:{start_line}:{line}:{path}")
            }
            ComposerKey::File { path } => write!(f, "file:{path}"),
            ComposerKey::Review => f.write_str("review"),
            ComposerKey::Reply { thread_id } => write!(f, "reply:{thread_id}"),
            ComposerKey::Edit { comment_id } => write!(f, "edit:{comment_id}"),
        }
    }
}

/// The human as an author (stored as `you`, T1.13).
pub fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".to_owned(),
        session_id: None,
    }
}

/// What the tab shows, for saving a new thread against it.
#[derive(Debug, Clone)]
pub struct DiffShown {
    pub review_id: String,
    pub diff_id: DiffId,
    pub repo: RepoInfo,
    /// The displayed base and the live state, for live tabs (pinned first).
    pub live: Option<(ResolvedSide, LiveState)>,
    /// The live snapshot's scratch object store (blobs of the working tree).
    pub scratch: Option<PathBuf>,
}

/// One save.
#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub key: ComposerKey,
    pub body: String,
    pub shown: DiffShown,
}

/// What a save did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// The iteration a live diff was pinned as first.
    pub pinned: Option<IterationInfo>,
    /// The thread written to (created, replied to; `None` for an edit).
    pub thread_id: Option<String>,
    /// The comment created (`None` for an edit).
    pub comment_id: Option<String>,
}

/// Saves `req` as a draft: a new thread (on a live diff after pinning it),
/// a reply, or an edit of one's own comment. Blocking (store writes, and for
/// a new thread a snapshot pin and blob reads): run it on the background
/// executor.
pub fn save(core: &Core, req: &SaveRequest) -> anyhow::Result<Saved> {
    let body = req.body.trim_end().to_owned();
    match &req.key {
        ComposerKey::Reply { thread_id } => {
            let comment = core.reply(thread_id, &body, &human())?;
            Ok(Saved {
                pinned: None,
                thread_id: Some(thread_id.clone()),
                comment_id: Some(comment),
            })
        }
        ComposerKey::Edit { comment_id } => {
            core.edit_comment(comment_id, &body, &human())?;
            Ok(Saved {
                pinned: None,
                thread_id: None,
                comment_id: None,
            })
        }
        key => {
            let subject = key.subject().expect("a new-thread key");
            let shown = &req.shown;
            let (pinned, diff_id) = match &shown.live {
                Some((base, state)) => {
                    let it = core.pin_live_on_base(
                        &shown.review_id,
                        base,
                        state,
                        PinnedBy::Comment,
                        &Actor::human(),
                    )?;
                    let diff_id = it.diff_id.clone();
                    (Some(it), diff_id)
                }
                None => (None, shown.diff_id.clone()),
            };
            let blobs = BlobReader::open(&shown.repo)?;
            let blobs = match &shown.scratch {
                Some(dir) => blobs.with_scratch(dir)?,
                None => blobs,
            };
            let thread = core.create_thread(
                &NewThread {
                    review_id: shown.review_id.clone(),
                    diff_id,
                    subject,
                    kind: ThreadKind::Comment,
                    body_md: body,
                    author: human(),
                },
                &blobs,
            )?;
            let comment = core
                .thread(&thread, polygloss_core::review::Viewer::Human)
                .ok()
                .and_then(|t| t.comments.first().map(|c| c.id.clone()));
            Ok(Saved {
                pinned,
                thread_id: Some(thread),
                comment_id: comment,
            })
        }
    }
}
