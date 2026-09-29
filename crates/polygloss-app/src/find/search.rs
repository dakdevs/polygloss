//! The search behind ⌘F (design §11.14), free of GPUI state: compile a
//! query, find its matches in one file's two blobs, and order them the way
//! the diff shows them.
//!
//! Matching is per line (a match never spans lines), over bytes, so
//! non-UTF-8 content is searched too. A file is read through the
//! [`DiffProvider`], the same blobs the viewport reads, whether the
//! viewport has loaded the file or not. Only files with rows to go to are
//! searched: text and symlinks whose content changed (binary, submodule and
//! content-unchanged renames are skipped, and so is a text file whose blobs
//! turn out to hold a NUL byte, like the viewport's first-read rule).
//!
//! A line that is the same on both sides (context) is one match, on the new
//! side, carrying its old line so a hidden one can be revealed (context is
//! revealed by old lines, `polygloss_diff::rows::Expansions`). Removed lines
//! sort before the lines that replace them, as in the unified view.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::SharedString;
use polygloss_diff::hunks::{Block, FileDiff, diff_blobs};
use polygloss_diff::line_map::{LineMap, Mapped};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{FileChange, FileKind, Side};
use polygloss_viewport::DiffProvider;
use polygloss_viewport::materialize::is_binary;
use polygloss_viewport::special::is_lfs_pointer;
use regex::bytes::{Regex, RegexBuilder};

/// Lines of context revealed on each side of a match in hidden context.
pub const REVEAL_CONTEXT: u32 = 3;

/// Bytes of a line shown before a match in a preview (the result list is
/// a sidebar: the match must stay in view).
const PREVIEW_BEFORE: usize = 16;
/// Bytes of a preview at most (longer lines are cut around the match).
const PREVIEW_LEN: usize = 160;

/// The find bar's toggles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Match case (off: case-insensitive, as browsers and GitHub search).
    pub case_sensitive: bool,
    /// The query is a regular expression (`regex` crate syntax, OQ-P8).
    pub regex: bool,
}

/// One match: a line of one side of one file, and what the result list
/// shows for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindMatch {
    pub file_idx: u32,
    /// `New` for added and context lines, `Old` for removed ones.
    pub side: Side,
    /// 0-based line of `side`.
    pub line: u32,
    /// For a context line, the same line on the old side (reveals go by
    /// old lines); `None` for added and removed lines.
    pub old_line: Option<u32>,
    /// Hidden between hunks when searched (nothing revealed).
    pub hidden: bool,
    /// The line, whitespace-trimmed and cut around the match, tabs as
    /// spaces, lossy UTF-8.
    pub preview: SharedString,
    /// The match's byte range in `preview`.
    pub preview_match: Range<usize>,
}

/// The regex for `query`: `None` for an empty query, an error message for
/// an invalid pattern. Literal text is escaped; case-insensitive unless
/// asked.
pub fn compile(query: &str, opts: FindOptions) -> Result<Option<Regex>, String> {
    if query.is_empty() {
        return Ok(None);
    }
    let pattern = if opts.regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!opts.case_sensitive)
        // A match never spans lines; `^`/`$` are a line's ends.
        .multi_line(false)
        .build()
        .map(Some)
        .map_err(|e| match e {
            regex::Error::Syntax(s) => s.lines().last().unwrap_or(&s).trim().to_owned(),
            e => e.to_string(),
        })
}

/// Whether file `change` has rows to search (and so to go to).
pub fn searchable(change: &FileChange) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink) && change.old_blob != change.new_blob
}

/// The matches of `re` in file `change`, read through `provider`, in
/// display order. Empty when the file has nothing to search, a blob cannot
/// be read (logged), or `cancel` is set.
pub fn search_file(
    change: &FileChange,
    provider: &dyn DiffProvider,
    re: &Regex,
    diff: &DiffOptions,
    cancel: &AtomicBool,
) -> Vec<FindMatch> {
    if !searchable(change) || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let read = |oid: &polygloss_diff::Oid| -> Option<std::sync::Arc<[u8]>> {
        if oid.is_zero() {
            return Some(std::sync::Arc::from(&[][..]));
        }
        match provider.load_blob(oid) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!("find: reading {} ({oid}): {e:#}", change.display_path());
                None
            }
        }
    };
    let (Some(old), Some(new)) = (read(&change.old_blob), read(&change.new_blob)) else {
        return Vec::new();
    };
    if cancel.load(Ordering::Relaxed)
        || is_binary(&old)
        || is_binary(&new)
        || (is_lfs_pointer(&old) && is_lfs_pointer(&new))
    {
        return Vec::new();
    }
    search_blobs(change.idx, &old, &new, re, diff)
}

/// The matches of `re` in the blobs `old` and `new` of file `file_idx`, in
/// display order (see the module docs). `diff` groups hunks as the viewport
/// does, which decides what context is hidden.
pub fn search_blobs(
    file_idx: u32,
    old: &[u8],
    new: &[u8],
    re: &Regex,
    diff: &DiffOptions,
) -> Vec<FindMatch> {
    // Most files do not match at all: skip the line diff for them.
    if !re.is_match(old) && !re.is_match(new) {
        return Vec::new();
    }
    let fd = diff_blobs(old, new, diff);
    let map = LineMap::from_diff(&fd);
    let mut out: Vec<((u32, u8, u32), FindMatch)> = Vec::new();
    for line in 0..fd.new.len() {
        let text = fd.new.line(new, line);
        let old_line = match map.map_line_back(line) {
            Mapped::Unchanged(o) => Some(o),
            Mapped::Changed { .. } => None,
        };
        let hidden = old_line.is_some_and(|o| is_hidden(&fd, &[], o));
        for m in line_matches(re, text) {
            let (preview, preview_match) = preview(text, m);
            out.push((
                (line, 1, 0),
                FindMatch {
                    file_idx,
                    side: Side::New,
                    line,
                    old_line,
                    hidden,
                    preview,
                    preview_match,
                },
            ));
        }
    }
    // Removed lines: the old lines of change blocks, placed right before
    // the new lines that replace them (or the line after the removal).
    for (old_range, new_start) in removed_blocks(&fd) {
        for line in old_range {
            let text = fd.old.line(old, line);
            for m in line_matches(re, text) {
                let (preview, preview_match) = preview(text, m);
                out.push((
                    (new_start, 0, line),
                    FindMatch {
                        file_idx,
                        side: Side::Old,
                        line,
                        old_line: None,
                        hidden: false,
                        preview,
                        preview_match,
                    },
                ));
            }
        }
    }
    // Stable: several matches in one line keep their order.
    out.sort_by_key(|(key, _)| *key);
    out.into_iter().map(|(_, m)| m).collect()
}

/// Whether old line `old_line` is hidden: outside every hunk of `diff` and
/// every revealed range (`[start, end)` old lines, as
/// `DiffViewport::expansions` gives them).
pub fn is_hidden(diff: &FileDiff, expansions: &[[u32; 2]], old_line: u32) -> bool {
    let in_hunk = diff.hunks.iter().any(|h| h.old.contains(&old_line));
    let revealed = expansions
        .iter()
        .any(|&[start, end]| start <= old_line && old_line < end);
    !in_hunk && !revealed
}

/// The old lines to reveal for a match on hidden old line `old_line`: the
/// line and [`REVEAL_CONTEXT`] lines around it.
pub fn reveal_range(old_line: u32) -> [u32; 2] {
    [
        old_line.saturating_sub(REVEAL_CONTEXT),
        old_line.saturating_add(REVEAL_CONTEXT + 1),
    ]
}

/// Every change block with removed lines: its old lines and the new line
/// its replacement (or the line after the removal) starts at.
fn removed_blocks(fd: &FileDiff) -> impl Iterator<Item = (Range<u32>, u32)> + '_ {
    fd.hunks
        .iter()
        .flat_map(|h| &h.blocks)
        .filter_map(|b| match b {
            Block::Change { old, new } if !old.is_empty() => Some((old.clone(), new.start)),
            _ => None,
        })
}

/// The non-empty matches of `re` in one line (without its `\r`).
fn line_matches<'a>(re: &'a Regex, line: &'a [u8]) -> impl Iterator<Item = Range<usize>> + 'a {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    re.find_iter(line)
        .filter(|m| !m.is_empty())
        .map(|m| m.range())
}

/// A line's preview around match `m` (byte offsets in `line`) and the
/// match's range in it.
fn preview(line: &[u8], m: Range<usize>) -> (SharedString, Range<usize>) {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut start = m.start.saturating_sub(PREVIEW_BEFORE);
    // Leading indentation says nothing in a list.
    while start < m.start && matches!(line[start], b' ' | b'\t') {
        start += 1;
    }
    let mut end = (start + PREVIEW_LEN).max(m.end).min(line.len());
    // Whole UTF-8 characters: a cut never starts or ends inside one.
    while start > 0 && start < m.start && is_continuation(line[start]) {
        start -= 1;
    }
    while end < line.len() && is_continuation(line[end]) {
        end += 1;
    }
    let text = |r: Range<usize>| String::from_utf8_lossy(&line[r]).replace('\t', " ");
    let before = text(start..m.start);
    let matched = text(m.clone());
    let after = text(m.end..end);
    // Cut text is marked; skipped indentation is not.
    let cut = line[..start].iter().any(|b| !matches!(b, b' ' | b'\t'));
    let lead = if cut { "…" } else { "" };
    let tail = if end < line.len() { "…" } else { "" };
    let at = lead.len() + before.len();
    let preview = format!("{lead}{before}{matched}{after}{tail}");
    (preview.into(), at..at + matched.len())
}

fn is_continuation(b: u8) -> bool {
    b & 0b1100_0000 == 0b1000_0000
}
