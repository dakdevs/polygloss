//! `LineMap`: blob-to-blob line mapping for carry-forward and scroll anchors (T1.8).
//!
//! Used by comment carry-forward (design §8.6, ADR-0010), refresh scroll-anchor
//! restore (ADR-0009) and open-in-editor (ADR-0021). The map holds only the
//! changed line ranges of one line diff; every line outside them is in an equal
//! region and maps by a constant offset. Lookups are binary searches, so a map
//! costs one diff to build and `O(log changes)` per query.

use std::ops::Range;

use crate::hunks::{Block, FileDiff, line_changes};
use crate::options::DiffOptions;

/// Where one line lands on the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mapped {
    /// The line is in an equal region; this is its line number on the other side.
    Unchanged(u32),
    /// The line was changed, removed or never existed. `nearest` is the closest
    /// line on the other side: the line at the same offset inside the
    /// replacement block (clamped to its last line), else the line right after
    /// the removed block, clamped to the last line (0 when that side is empty).
    Changed { nearest: u32 },
}

/// Where an inclusive line range lands on the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MappedRange {
    /// Every line is unchanged and they are still contiguous; `end` is inclusive.
    Moved { start: u32, end: u32 },
    /// Some line changed, or new lines were inserted between the range's lines.
    /// `nearest` is where the range's last line maps (its [`Mapped`] target or
    /// nearest line), since that is where a thread renders.
    Outdated { nearest: u32 },
}

/// A changed region: `old` lines were replaced by `new` lines. Either range may
/// be empty, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Change {
    old: Range<u32>,
    new: Range<u32>,
}

/// Maps 0-based line numbers between two blobs through their line diff.
///
/// [`LineMap::new`] diffs with Myers plus the indent heuristic, whitespace
/// exact (the §6.3 defaults, so a re-indented line counts as changed).
/// [`LineMap::from_diff`] reuses the alignment of an existing [`FileDiff`],
/// whatever its options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMap {
    /// Changed regions in order; both their old and their new ranges ascend and
    /// never touch (imara separates hunks by at least one equal line).
    changes: Vec<Change>,
    old_len: u32,
    new_len: u32,
}

#[derive(Clone, Copy)]
enum Direction {
    OldToNew,
    NewToOld,
}

impl LineMap {
    /// Diffs `old` against `new` (Myers, indent heuristic, whitespace exact).
    pub fn new(old: &[u8], new: &[u8]) -> LineMap {
        let changes = line_changes(old, new, &DiffOptions::default())
            .into_iter()
            .map(|(old, new)| Change { old, new })
            .collect();
        LineMap {
            changes,
            old_len: line_count(old),
            new_len: line_count(new),
        }
    }

    /// The map of an existing diff: its `Change` blocks, independent of the
    /// context it was grouped with. In whitespace mode, lines that differ only
    /// in whitespace count as unchanged, as the diff shows them.
    pub fn from_diff(fd: &FileDiff) -> LineMap {
        let changes = fd
            .hunks
            .iter()
            .flat_map(|h| &h.blocks)
            .filter_map(|b| match b {
                Block::Change { old, new } => Some(Change {
                    old: old.clone(),
                    new: new.clone(),
                }),
                Block::Equal { .. } => None,
            })
            .collect();
        LineMap {
            changes,
            old_len: fd.old.len(),
            new_len: fd.new.len(),
        }
    }

    /// Number of lines on the old side.
    pub fn old_len(&self) -> u32 {
        self.old_len
    }

    /// Number of lines on the new side.
    pub fn new_len(&self) -> u32 {
        self.new_len
    }

    /// Maps an old line to the new side. Lines past the end of the old blob
    /// report `Changed` with the last new line as `nearest`.
    pub fn map_line(&self, old_line: u32) -> Mapped {
        self.map(old_line, Direction::OldToNew)
    }

    /// Maps the old lines `start..=end_inclusive` (bounds in either order).
    /// `Moved` only if every line is in one equal region, so the lines stay
    /// unchanged and contiguous; otherwise `Outdated`.
    pub fn map_range(&self, start: u32, end_inclusive: u32) -> MappedRange {
        let (lo, hi) = (start.min(end_inclusive), start.max(end_inclusive));
        let outdated = || MappedRange::Outdated {
            nearest: match self.map_line(hi) {
                Mapped::Unchanged(line) => line,
                Mapped::Changed { nearest } => nearest,
            },
        };
        if hi >= self.old_len {
            return outdated();
        }
        // The first change that ends after `lo`: a changed line inside the
        // range, or an insertion between two of its lines, starts at or
        // before `hi`. (An insertion right before `lo` has `old.end == lo`.)
        let idx = self.changes.partition_point(|c| c.old.end <= lo);
        if self.changes.get(idx).is_some_and(|c| c.old.start <= hi) {
            return outdated();
        }
        let start = self.shifted(lo, idx, Direction::OldToNew);
        MappedRange::Moved {
            start,
            end: start + (hi - lo),
        }
    }

    /// Maps a new line back to the old side (scroll anchors). Lines past the
    /// end of the new blob report `Changed` with the last old line as `nearest`.
    pub fn map_line_back(&self, new_line: u32) -> Mapped {
        self.map(new_line, Direction::NewToOld)
    }

    fn map(&self, line: u32, dir: Direction) -> Mapped {
        let (from_len, to_len) = match dir {
            Direction::OldToNew => (self.old_len, self.new_len),
            Direction::NewToOld => (self.new_len, self.old_len),
        };
        let last = to_len.saturating_sub(1);
        if line >= from_len {
            return Mapped::Changed { nearest: last };
        }
        let idx = self.changes.partition_point(|c| src(c, dir).end <= line);
        if let Some(c) = self.changes.get(idx)
            && src(c, dir).start <= line
        {
            let (from, to) = (src(c, dir), dst(c, dir));
            let nearest = if to.is_empty() {
                to.start
            } else {
                to.start + (line - from.start).min(to.end - to.start - 1)
            };
            return Mapped::Changed {
                nearest: nearest.min(last),
            };
        }
        Mapped::Unchanged(self.shifted(line, idx, dir))
    }

    /// Maps an unchanged `line` given `idx`, the number of changes that end at
    /// or before it: the offset after the last of those changes.
    fn shifted(&self, line: u32, idx: usize, dir: Direction) -> u32 {
        match idx.checked_sub(1).map(|i| &self.changes[i]) {
            None => line,
            Some(c) => dst(c, dir).end + (line - src(c, dir).end),
        }
    }
}

fn src(c: &Change, dir: Direction) -> &Range<u32> {
    match dir {
        Direction::OldToNew => &c.old,
        Direction::NewToOld => &c.new,
    }
}

fn dst(c: &Change, dir: Direction) -> &Range<u32> {
    match dir {
        Direction::OldToNew => &c.new,
        Direction::NewToOld => &c.old,
    }
}

/// Line count as imara's `byte_lines` and [`crate::lines::LineIndex`] see it: a
/// final line without `\n` counts.
fn line_count(bytes: &[u8]) -> u32 {
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    let partial = usize::from(bytes.last().is_some_and(|&b| b != b'\n'));
    (newlines + partial) as u32
}
