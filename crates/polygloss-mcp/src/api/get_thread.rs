//! `get_thread` (design §15.2): one thread with its anchor, snippets, GitHub-style
//! `diff_hunk`, comments with structured suggestions, and who resolved it.
//!
//! - The position (and `current_snippet`) is relative to the review's latest
//!   iteration (the origin diff for a thread without a review iteration).
//! - `original_snippet` is the anchored lines as they were when the thread was
//!   made; `current_snippet` the lines at the position now, with 3 lines of
//!   context on each side when the thread is outdated.
//! - `diff_hunk` follows GitHub: in the diff the thread was made on, the hunk
//!   header through the commented line (a range's last line). A line outside
//!   every hunk (anchored in expanded context) gets a context-only hunk of up
//!   to 3 lines before it.
//! - Suggestions apply to new-side line threads only (§8.5): each
//!   ```` ```suggestion ```` block replaces `start_line..=line`, the position's
//!   lines while the thread is exact or moved, else the anchor's; `original` is
//!   the anchored text.
//! - Bodies over 20k characters are cut and flagged `truncated`.

use polygloss_core::review::{
    AuthorKind, CommentView, Position, PositionState, Subject, ThreadView, Viewer,
    parse_suggestions,
};
use polygloss_diff::hunks::{Block, diff_blobs};
use polygloss_diff::lines::LineIndex;
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{FileChange, FileKind, FileStatus, Oid, Side};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::shapes::{BODY_MAX_CHARS, SideParam, ThreadSummary, timestamp, truncate_chars};
use crate::api::thread_summary::{
    DiffContext, latest_diff, positions, summarize, takes_suggestions,
};
use crate::context::ApiContext;
use crate::errors::ApiError;

/// Lines of context around an outdated thread's `current_snippet`, and before a
/// line outside every hunk in its `diff_hunk`.
const CONTEXT: u32 = 3;

/// `get_thread` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetThreadRequest {
    pub thread_id: String,
}

/// `get_thread` result: the thread's summary plus its details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GetThreadResult {
    #[serde(flatten)]
    pub summary: ThreadSummary,
    pub anchor: AnchorOut,
    pub comments: Vec<CommentOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<ResolvedByOut>,
    pub origin_diff_id: String,
}

/// The anchor as created (empty for review threads, `{path}` for file threads).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AnchorOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<SideParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The blob the line numbers refer to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_blob: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_hunk: Option<String>,
}

/// One comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommentOut {
    pub comment_id: String,
    pub author_kind: AuthorKind,
    pub author_name: String,
    /// Empty for a deleted root's placeholder.
    pub body_md: String,
    /// The body was longer than 20k characters and was cut.
    pub truncated: bool,
    /// A "comment deleted" placeholder (a deleted root that has replies).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
    pub suggestions: Vec<SuggestionOut>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
}

/// A ```` ```suggestion ```` block: replace lines `start_line..=line` of the
/// file's new side (`original`) with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuggestionOut {
    pub start_line: u32,
    pub line: u32,
    pub original: String,
    pub replacement: String,
}

/// Who resolved the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedByOut {
    pub kind: AuthorKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub at: String,
}

/// One thread with its anchor, snippets, `diff_hunk` and comments.
pub fn get_thread(ctx: &ApiContext, req: GetThreadRequest) -> Result<GetThreadResult, ApiError> {
    let t = ctx.core.thread(req.thread_id.trim(), Viewer::Agent)?;
    let target = latest_diff(ctx, t.review_id.as_deref())?;
    let (pos, mut diffs) = positions(ctx, &[&t], target.as_ref())?;
    let position = pos.get(&t.id);
    let summary = summarize(&t, position);

    let target_id = target.unwrap_or_else(|| t.origin_diff_id.clone());
    if !diffs.contains_key(&t.origin_diff_id) && matches!(t.anchor.subject, Subject::Line { .. }) {
        let origin = DiffContext::load(ctx, &t.origin_diff_id)?;
        diffs.insert(t.origin_diff_id.clone(), origin);
    }
    let anchor = anchor_out(
        &t,
        position,
        diffs.get(&target_id),
        diffs.get(&t.origin_diff_id),
    );
    let comments = comments_out(&t, position, anchor.original_snippet.as_deref());
    Ok(GetThreadResult {
        summary,
        anchor,
        comments,
        resolved_by: t.resolved_by.as_ref().map(|r| ResolvedByOut {
            kind: r.kind,
            name: r.name.clone(),
            at: timestamp(r.at),
        }),
        origin_diff_id: t.origin_diff_id.as_str().to_owned(),
    })
}

fn anchor_out(
    t: &ThreadView,
    position: Option<&Position>,
    target: Option<&DiffContext>,
    origin: Option<&DiffContext>,
) -> AnchorOut {
    let (path, side, start_line, line) = match &t.anchor.subject {
        Subject::Review => return AnchorOut::default(),
        Subject::File { path } => {
            return AnchorOut {
                path: Some(path.clone()),
                ..AnchorOut::default()
            };
        }
        Subject::Line {
            path,
            side,
            start_line,
            line,
        } => (path, *side, *start_line, *line),
    };
    let original = original_snippet(t, start_line, line, origin);
    let current = position
        .zip(target)
        .and_then(|(p, dc)| current_snippet(p, dc));
    let diff_hunk = origin.and_then(|dc| {
        let file = origin_file(&dc.files, path)?;
        let (old, new) = file_blobs(file, dc)?;
        diff_hunk(&old, &new, side, line)
    });
    AnchorOut {
        path: Some(path.clone()),
        side: Some(side.into()),
        start_line: Some(start_line),
        line: Some(line),
        anchor_blob: t.anchor.anchor_blob.as_ref().map(|b| b.as_str().to_owned()),
        original_snippet: original,
        current_snippet: current,
        diff_hunk,
    }
}

/// The anchored lines as created: cut from the stored snippet (which carries 3
/// lines of context on each side), else read from the anchor blob.
fn original_snippet(
    t: &ThreadView,
    start_line: u32,
    line: u32,
    origin: Option<&DiffContext>,
) -> Option<String> {
    if let Some(snippet) = &t.anchor.anchor_snippet {
        let from = start_line.saturating_sub(CONTEXT).max(1);
        let lines: Vec<&str> = snippet.split('\n').collect();
        let (a, b) = ((start_line - from) as usize, (line - from) as usize);
        if start_line >= 1 && b < lines.len() && a <= b {
            return Some(lines[a..=b].join("\n"));
        }
    }
    let blob = t.anchor.anchor_blob.as_ref()?;
    let bytes = origin?.blobs.read(blob).ok()?;
    lines_text(&bytes, start_line, line)
}

/// The lines at the position now (widened by [`CONTEXT`] when outdated).
fn current_snippet(p: &Position, dc: &DiffContext) -> Option<String> {
    let (path, side, start, end) = (p.path.as_deref()?, p.side?, p.start_line?, p.line?);
    let file = dc.file(path)?;
    let blob = side_blob(file, side)?;
    let bytes = dc.blobs.read(blob).ok()?;
    let (start, end) = if p.state == PositionState::Outdated {
        (
            start.saturating_sub(CONTEXT).max(1),
            end.saturating_add(CONTEXT),
        )
    } else {
        (start, end)
    };
    let n = LineIndex::new(&bytes).len();
    lines_text(&bytes, start, end.min(n))
}

/// The blob of `file` on `side`, `None` when that side has none.
fn side_blob(file: &FileChange, side: Side) -> Option<&Oid> {
    let blob = match side {
        Side::Old => &file.old_blob,
        Side::New => &file.new_blob,
    };
    (!blob.is_zero()).then_some(blob)
}

/// Lines `from..=to` (1-based) joined with `\n`, each without a final `\r`,
/// invalid UTF-8 replaced; `None` when out of range.
fn lines_text(bytes: &[u8], from: u32, to: u32) -> Option<String> {
    let index = LineIndex::new(bytes);
    if from == 0 || from > to || to > index.len() {
        return None;
    }
    let lines: Vec<String> = (from - 1..to)
        .map(|i| {
            let l = index.line(bytes, i);
            String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)).into_owned()
        })
        .collect();
    Some(lines.join("\n"))
}

/// The origin diff's file a thread on `path` was made on: by display path,
/// else by old path (a renamed or deleted file's old name).
fn origin_file<'a>(files: &'a [FileChange], path: &str) -> Option<&'a FileChange> {
    files.iter().find(|f| f.display_path() == path).or_else(|| {
        files
            .iter()
            .find(|f| f.old_path.as_ref().is_some_and(|p| p.text == path))
    })
}

/// Both sides' bytes of a text file (an absent side is empty); `None` for
/// binaries, submodules and unreadable blobs.
fn file_blobs(file: &FileChange, dc: &DiffContext) -> Option<(Vec<u8>, Vec<u8>)> {
    if matches!(file.kind, FileKind::Binary | FileKind::Submodule) {
        return None;
    }
    let read = |side: Side| -> Option<Vec<u8>> {
        let absent = matches!(
            (side, file.status),
            (Side::Old, FileStatus::Added) | (Side::New, FileStatus::Deleted)
        );
        match side_blob(file, side) {
            Some(blob) if !absent => dc.blobs.read(blob).ok().map(|b| b.to_vec()),
            _ => Some(Vec::new()),
        }
    };
    Some((read(Side::Old)?, read(Side::New)?))
}

/// GitHub's `diff_hunk`: the header of the hunk containing line `line`
/// (1-based) of `side`, then its lines through that one. A line in no hunk gets
/// a context-only hunk of up to [`CONTEXT`] lines before it. `None` when the
/// line is not in the blob.
pub fn diff_hunk(old: &[u8], new: &[u8], side: Side, line: u32) -> Option<String> {
    let fd = diff_blobs(old, new, &DiffOptions::default());
    let (this, _) = match side {
        Side::Old => (&fd.old, &fd.new),
        Side::New => (&fd.new, &fd.old),
    };
    if line == 0 || line > this.len() {
        return None;
    }
    let target = line - 1;
    let mut out = String::new();
    fn push(out: &mut String, prefix: char, index: &LineIndex, bytes: &[u8], i: u32) {
        let l = index.line(bytes, i);
        out.push('\n');
        out.push(prefix);
        out.push_str(&String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)));
    }
    let on_side = |old_r: &std::ops::Range<u32>, new_r: &std::ops::Range<u32>| match side {
        Side::Old => old_r.clone(),
        Side::New => new_r.clone(),
    };
    let hunk = fd
        .hunks
        .iter()
        .find(|h| on_side(&h.old, &h.new).contains(&target));
    let Some(hunk) = hunk else {
        // Outside every hunk both sides are equal, shifted by the hunks before.
        let delta: i64 = fd
            .hunks
            .iter()
            .rfind(|h| on_side(&h.old, &h.new).end <= target)
            .map_or(0, |h| i64::from(h.new.end) - i64::from(h.old.end));
        let from = target.saturating_sub(CONTEXT);
        let count = target - from + 1;
        let (old_from, new_from) = match side {
            Side::Old => (i64::from(from), i64::from(from) + delta),
            Side::New => (i64::from(from) - delta, i64::from(from)),
        };
        let header = format!(
            "@@ -{} +{} @@",
            range_text(old_from.max(0) as u32, count),
            range_text(new_from.max(0) as u32, count)
        );
        out.push_str(&header);
        for i in new_from.max(0) as u32..new_from.max(0) as u32 + count {
            push(&mut out, ' ', &fd.new, new, i);
        }
        return Some(out);
    };
    out.push_str(&format!(
        "@@ -{} +{} @@",
        range_text(hunk.old.start, hunk.old.end - hunk.old.start),
        range_text(hunk.new.start, hunk.new.end - hunk.new.start)
    ));
    for block in &hunk.blocks {
        match block {
            Block::Equal { old: o, new: n } => {
                for k in 0..(n.end - n.start) {
                    push(&mut out, ' ', &fd.new, new, n.start + k);
                    let here = match side {
                        Side::Old => o.start + k,
                        Side::New => n.start + k,
                    };
                    if here == target {
                        return Some(out);
                    }
                }
            }
            Block::Change { old: o, new: n } => {
                for i in o.clone() {
                    push(&mut out, '-', &fd.old, old, i);
                    if side == Side::Old && i == target {
                        return Some(out);
                    }
                }
                for i in n.clone() {
                    push(&mut out, '+', &fd.new, new, i);
                    if side == Side::New && i == target {
                        return Some(out);
                    }
                }
            }
        }
    }
    Some(out)
}

/// git's hunk header range: 1-based start and `,count` unless the count is 1;
/// an empty range is printed as the line before it.
fn range_text(start0: u32, count: u32) -> String {
    let start = if count == 0 { start0 } else { start0 + 1 };
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

fn comments_out(
    t: &ThreadView,
    position: Option<&Position>,
    original: Option<&str>,
) -> Vec<CommentOut> {
    let lines = suggestion_lines(t, position);
    t.comments
        .iter()
        .map(|c| comment_out(c, lines, original))
        .collect()
}

/// The lines suggestions replace: the position's while exact or moved, else
/// the anchor's; `None` when suggestions do not apply.
fn suggestion_lines(t: &ThreadView, position: Option<&Position>) -> Option<(u32, u32)> {
    if !takes_suggestions(t) {
        return None;
    }
    let Subject::Line {
        start_line, line, ..
    } = &t.anchor.subject
    else {
        return None;
    };
    match position {
        Some(Position {
            state: PositionState::Exact | PositionState::Moved,
            start_line: Some(s),
            line: Some(l),
            ..
        }) => Some((*s, *l)),
        _ => Some((*start_line, *line)),
    }
}

fn comment_out(c: &CommentView, lines: Option<(u32, u32)>, original: Option<&str>) -> CommentOut {
    let (body_md, truncated) = truncate_chars(&c.body_md, BODY_MAX_CHARS);
    let suggestions = match lines {
        Some((start_line, line)) if !c.deleted => parse_suggestions(&c.body_md)
            .into_iter()
            .map(|replacement| SuggestionOut {
                start_line,
                line,
                original: original.unwrap_or_default().to_owned(),
                replacement,
            })
            .collect(),
        _ => Vec::new(),
    };
    CommentOut {
        comment_id: c.id.clone(),
        author_kind: c.author.kind,
        author_name: c.author.name.clone(),
        body_md,
        truncated,
        deleted: c.deleted,
        suggestions,
        created_at: timestamp(c.published_at.unwrap_or(c.created_at)),
        edited_at: c.edited_at.map(timestamp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEN: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
    const TEN_L5: &str = "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\nl10\n";

    fn hunk(old: &str, new: &str, side: Side, line: u32) -> Option<String> {
        diff_hunk(old.as_bytes(), new.as_bytes(), side, line)
    }

    #[test]
    fn diff_hunk_runs_from_the_header_through_the_line() {
        assert_eq!(
            hunk(TEN, TEN_L5, Side::New, 5).unwrap(),
            "@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5\n+L5"
        );
        assert_eq!(
            hunk(TEN, TEN_L5, Side::Old, 5).unwrap(),
            "@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5"
        );
        assert_eq!(
            hunk(TEN, TEN_L5, Side::New, 3).unwrap(),
            "@@ -2,7 +2,7 @@\n l2\n l3"
        );
        assert_eq!(
            hunk(TEN, TEN_L5, Side::New, 7).unwrap(),
            "@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5\n+L5\n l6\n l7"
        );
    }

    #[test]
    fn diff_hunk_outside_every_hunk_is_context_before_the_line() {
        // Two lines added at the top shift the new side by 2.
        let new = format!("n1\nn2\n{TEN}");
        assert_eq!(
            hunk(TEN, &new, Side::New, 12).unwrap(),
            "@@ -7,4 +9,4 @@\n l7\n l8\n l9\n l10"
        );
        assert_eq!(
            hunk(TEN, &new, Side::Old, 10).unwrap(),
            "@@ -7,4 +9,4 @@\n l7\n l8\n l9\n l10"
        );
        assert_eq!(hunk(TEN, TEN, Side::New, 1).unwrap(), "@@ -1 +1 @@\n l1");
    }

    #[test]
    fn diff_hunk_of_added_and_deleted_files() {
        assert_eq!(
            hunk("", "a\nb\n", Side::New, 2).unwrap(),
            "@@ -0,0 +1,2 @@\n+a\n+b"
        );
        assert_eq!(
            hunk("a\nb\n", "", Side::Old, 1).unwrap(),
            "@@ -1,2 +0,0 @@\n-a"
        );
    }

    #[test]
    fn diff_hunk_rejects_lines_outside_the_blob() {
        assert_eq!(hunk(TEN, TEN_L5, Side::New, 0), None);
        assert_eq!(hunk(TEN, TEN_L5, Side::New, 11), None);
        assert_eq!(hunk("", "a\n", Side::Old, 1), None);
    }

    #[test]
    fn lines_text_strips_carriage_returns() {
        assert_eq!(lines_text(b"a\r\nb\r\nc", 2, 3).unwrap(), "b\nc");
        assert_eq!(lines_text(b"a\n", 1, 2), None);
        assert_eq!(lines_text(b"a\n", 0, 1), None);
    }
}
