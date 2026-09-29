//! Split and unified row model, including gap and expander rows (T1.9, design §11.6).
//!
//! [`build_rows`] turns a [`FileDiff`] into the rows the viewport paints:
//! context, removed and added lines, one [`Row::Gap`] per run of unchanged lines
//! that is still hidden, and `\ No newline at end of file` markers. What is
//! hidden is decided by [`Expansions`], a set of revealed **old-side** line
//! ranges. Unchanged gaps map old lines to new lines one to one, so old lines
//! alone identify revealed context, and the same set can be applied to any diff
//! of the blob (e.g. after the whitespace toggle).

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::hunks::{Block, FileDiff};
use crate::lines::LineIndex;
use crate::types::Side;

/// Split (old left, new right) or unified (one column, two line-number columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Split,
    Unified,
}

/// What a displayed line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    /// Unchanged: a hunk's context or revealed gap lines.
    Context,
    /// Old side only.
    Removed,
    /// New side only.
    Added,
}

/// An unchanged gap: `GapId(i)` is the gap before hunk `i`, and
/// `GapId(hunks.len())` is the trailing gap after the last hunk. A file without
/// hunks has one gap, `GapId(0)`, spanning the whole file. Ids depend only on
/// the [`FileDiff`], never on what is revealed, so they stay stable while the
/// user expands context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GapId(pub u32);

/// A gap expander action: "↑20 / ↓20 / Expand all" (design §11.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpandBy {
    /// Reveal up to `n` hidden lines at the bottom of the gap, i.e. just above
    /// the content that follows it (GitHub's "expand up").
    Up(u32),
    /// Reveal up to `n` hidden lines at the top of the gap, i.e. just below the
    /// content before it (GitHub's "expand down").
    Down(u32),
    /// Reveal the whole gap.
    All,
}

/// Revealed context: a normalized set of 0-based, half-open **old-side** line
/// ranges (sorted, disjoint, non-adjacent, non-empty). Ranges may overlap hunks
/// or run past the end of the file; [`build_rows`] only looks at the part that
/// falls in a gap. This is what view state stores per file (design §11.12).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Expansions {
    ranges: Vec<Range<u32>>,
}

impl Expansions {
    /// Applies an expander action to gap `gap` of `fd`. When earlier reveals
    /// left several hidden runs in the gap, `Up` acts on the last one and `Down`
    /// on the first. A gap id past the trailing gap is ignored.
    pub fn expand(&mut self, fd: &FileDiff, gap: GapId, by: ExpandBy) {
        let Some(g) = gap_ranges(fd).into_iter().nth(gap.0 as usize) else {
            return;
        };
        let hidden = self.hidden_in(g.old.clone());
        let reveal = match by {
            ExpandBy::All => g.old,
            ExpandBy::Up(n) => match hidden.last() {
                Some(p) => p.end.saturating_sub(n).max(p.start)..p.end,
                None => return,
            },
            ExpandBy::Down(n) => match hidden.first() {
                Some(p) => p.start..p.start.saturating_add(n).min(p.end),
                None => return,
            },
        };
        self.reveal(reveal);
    }

    /// Reveals every line of `fd` (`E`, "Expand all" in the file menu).
    pub fn expand_file(&mut self, fd: &FileDiff) {
        self.reveal(0..fd.old.len());
    }

    /// Reveals the old lines in `old` (half-open). Empty ranges are ignored.
    /// Used for any reveal that is not a gap action, e.g. one hidden run of a
    /// split gap, or going to a find match inside collapsed context.
    pub fn reveal(&mut self, old: Range<u32>) {
        if old.start >= old.end {
            return;
        }
        // Ranges that overlap or touch `old` merge into it.
        let first = self.ranges.partition_point(|r| r.end < old.start);
        let last = self.ranges.partition_point(|r| r.start <= old.end);
        let merged = match self.ranges.get(first..last) {
            Some(touching) if !touching.is_empty() => {
                touching[0].start.min(old.start)..touching[touching.len() - 1].end.max(old.end)
            }
            _ => old,
        };
        self.ranges.splice(first..last, [merged]);
    }

    /// Whether nothing is revealed.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The revealed ranges as `[start, end)` pairs of 0-based old lines, sorted.
    pub fn to_ranges(&self) -> Vec<[u32; 2]> {
        self.ranges.iter().map(|r| [r.start, r.end]).collect()
    }

    /// Rebuilds from [`Expansions::to_ranges`] output, normalizing any input:
    /// sorts, merges overlapping or touching ranges, drops empty or reversed ones.
    pub fn from_ranges(r: &[[u32; 2]]) -> Expansions {
        let mut ranges: Vec<Range<u32>> = r
            .iter()
            .filter(|[start, end]| start < end)
            .map(|&[start, end]| start..end)
            .collect();
        ranges.sort_by_key(|r| r.start);
        let mut out: Vec<Range<u32>> = Vec::with_capacity(ranges.len());
        for r in ranges {
            match out.last_mut() {
                Some(prev) if r.start <= prev.end => prev.end = prev.end.max(r.end),
                _ => out.push(r),
            }
        }
        Expansions { ranges: out }
    }

    /// The maximal hidden runs inside `span`, in order.
    fn hidden_in(&self, span: Range<u32>) -> Vec<Range<u32>> {
        let mut hidden = Vec::new();
        let mut at = span.start;
        let first = self.ranges.partition_point(|r| r.end <= span.start);
        for r in &self.ranges[first..] {
            if r.start >= span.end {
                break;
            }
            if r.start > at {
                hidden.push(at..r.start);
            }
            at = at.max(r.end);
        }
        if at < span.end {
            hidden.push(at..span.end);
        }
        hidden
    }
}

/// One side of a split row. `pair` is the index into the change block's
/// [`crate::word::pair_lines`] list when the line is paired (for word diff).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cell {
    pub line: u32,
    pub kind: LineKind,
    pub pair: Option<u32>,
}

/// A displayed row. Line numbers are 0-based.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Row {
    /// A hidden run of unchanged lines with its expander. `can_up` is false when
    /// the run reaches the end of the file (nothing below to expand up from),
    /// `can_down` when it starts at line 0. Usually a gap has one hidden run; if
    /// revealed ranges split it, each run gets its own row with the same `id`.
    Gap {
        id: GapId,
        old: Range<u32>,
        new: Range<u32>,
        can_up: bool,
        can_down: bool,
    },
    /// A unified row: context has both line numbers, removed only `old`, added
    /// only `new`. A change block lists its removed lines, then its added lines.
    Unified {
        old: Option<u32>,
        new: Option<u32>,
        kind: LineKind,
        pair: Option<u32>,
    },
    /// A split row. A change block pairs removed and added lines row by row, in
    /// order (as `pair_lines` does); the shorter side's extra rows are `None`.
    Split {
        left: Option<Cell>,
        right: Option<Cell>,
    },
    /// `\ No newline at end of file` for `side`, right after the row showing
    /// that side's last line (unified) or after its change block (split). In
    /// unified, a context row whose sides both lack the newline gets one marker
    /// with `side: New`, as git prints it; in split, markers of both sides at
    /// the same place are one [`Row::NoNewlineBoth`]. Never emitted while the
    /// last line is hidden in a gap.
    NoNewline { side: Side },
    /// Split only: both sides' `\ No newline at end of file` markers on one
    /// row (each in its own half), after a context row or change block that
    /// holds both sides' last lines.
    NoNewlineBoth,
}

/// The rows of `fd` in `layout`, with the gaps `exp` reveals shown as context.
/// Cost is linear in the number of rows plus the number of gaps and revealed
/// ranges.
pub fn build_rows(fd: &FileDiff, exp: &Expansions, layout: Layout) -> Vec<Row> {
    let mut b = Builder {
        rows: Vec::with_capacity(estimate(fd)),
        layout,
        old_last: last_line_without_newline(&fd.old),
        new_last: last_line_without_newline(&fd.new),
        old_len: fd.old.len(),
    };
    let gaps = gap_ranges(fd);
    for (i, gap) in gaps.iter().enumerate() {
        b.gap(GapId(i as u32), gap, exp);
        if let Some(hunk) = fd.hunks.get(i) {
            for block in &hunk.blocks {
                match block {
                    Block::Equal { old, new } => b.context(old.clone(), new.start),
                    Block::Change { old, new } => b.change(old.clone(), new.clone()),
                }
            }
        }
    }
    b.rows
}

/// An unchanged gap's line ranges on both sides (same length).
struct Gap {
    old: Range<u32>,
    new: Range<u32>,
}

/// Every gap of `fd`, indexed by [`GapId`]: before each hunk, then the trailing
/// gap. Some may be empty.
fn gap_ranges(fd: &FileDiff) -> Vec<Gap> {
    let mut gaps = Vec::with_capacity(fd.hunks.len() + 1);
    let (mut old, mut new) = (0, 0);
    for hunk in &fd.hunks {
        gaps.push(Gap {
            old: old..hunk.old.start,
            new: new..hunk.new.start,
        });
        (old, new) = (hunk.old.end, hunk.new.end);
    }
    gaps.push(Gap {
        old: old..fd.old.len(),
        new: new..fd.new.len(),
    });
    gaps
}

fn last_line_without_newline(index: &LineIndex) -> Option<u32> {
    (!index.has_trailing_newline()).then(|| index.len() - 1)
}

/// Rows for the hunks alone plus one per gap; revealed context grows the vector.
fn estimate(fd: &FileDiff) -> usize {
    let lines: u32 = fd
        .hunks
        .iter()
        .map(|h| h.old.len().max(h.new.len()) as u32)
        .sum();
    lines as usize + fd.hunks.len() + 2
}

struct Builder {
    rows: Vec<Row>,
    layout: Layout,
    /// The old side's last line when it has no trailing newline.
    old_last: Option<u32>,
    new_last: Option<u32>,
    old_len: u32,
}

impl Builder {
    /// A gap's hidden runs as gap rows and its revealed lines as context.
    fn gap(&mut self, id: GapId, gap: &Gap, exp: &Expansions) {
        if gap.old.is_empty() {
            return;
        }
        let to_new = |old: u32| old - gap.old.start + gap.new.start;
        let mut at = gap.old.start;
        for hidden in exp.hidden_in(gap.old.clone()) {
            self.context(at..hidden.start, to_new(at));
            self.rows.push(Row::Gap {
                id,
                new: to_new(hidden.start)..to_new(hidden.end),
                can_up: hidden.end < self.old_len,
                can_down: hidden.start > 0,
                old: hidden.clone(),
            });
            at = hidden.end;
        }
        self.context(at..gap.old.end, to_new(at));
    }

    /// Context rows for old lines `old`, new lines starting at `new_start`.
    fn context(&mut self, old: Range<u32>, new_start: u32) {
        for (o, n) in old.zip(new_start..) {
            let cell = |line| Cell {
                line,
                kind: LineKind::Context,
                pair: None,
            };
            self.rows.push(match self.layout {
                Layout::Unified => Row::Unified {
                    old: Some(o),
                    new: Some(n),
                    kind: LineKind::Context,
                    pair: None,
                },
                Layout::Split => Row::Split {
                    left: Some(cell(o)),
                    right: Some(cell(n)),
                },
            });
            let old_marker = self.old_last == Some(o);
            let new_marker = self.new_last == Some(n);
            match self.layout {
                // One unified line, one marker (git prints the new side's).
                Layout::Unified if old_marker && new_marker => self.marker(Side::New),
                _ => self.split_markers(old_marker, new_marker),
            }
        }
    }

    /// A change block: removed then added lines (unified) or paired rows (split).
    fn change(&mut self, old: Range<u32>, new: Range<u32>) {
        let paired = old.len().min(new.len()) as u32;
        let pair = |k: u32| (k < paired).then_some(k);
        let old_marker = self.old_last.is_some_and(|l| old.contains(&l));
        let new_marker = self.new_last.is_some_and(|l| new.contains(&l));
        match self.layout {
            Layout::Unified => {
                for (k, o) in old.enumerate() {
                    self.rows.push(Row::Unified {
                        old: Some(o),
                        new: None,
                        kind: LineKind::Removed,
                        pair: pair(k as u32),
                    });
                }
                if old_marker {
                    self.marker(Side::Old);
                }
                for (k, n) in new.enumerate() {
                    self.rows.push(Row::Unified {
                        old: None,
                        new: Some(n),
                        kind: LineKind::Added,
                        pair: pair(k as u32),
                    });
                }
                if new_marker {
                    self.marker(Side::New);
                }
            }
            Layout::Split => {
                let height = old.len().max(new.len()) as u32;
                for k in 0..height {
                    let cell = |range: &Range<u32>, kind| {
                        (k < range.len() as u32).then(|| Cell {
                            line: range.start + k,
                            kind,
                            pair: pair(k),
                        })
                    };
                    self.rows.push(Row::Split {
                        left: cell(&old, LineKind::Removed),
                        right: cell(&new, LineKind::Added),
                    });
                }
                self.split_markers(old_marker, new_marker);
            }
        }
    }

    fn marker(&mut self, side: Side) {
        self.rows.push(Row::NoNewline { side });
    }

    /// The markers after a split row or change block: both on one row, or
    /// the one side's alone.
    fn split_markers(&mut self, old_marker: bool, new_marker: bool) {
        match (old_marker, new_marker) {
            (true, true) => self.rows.push(Row::NoNewlineBoth),
            (true, false) => self.marker(Side::Old),
            (false, true) => self.marker(Side::New),
            (false, false) => {}
        }
    }
}
