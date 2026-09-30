//! `FileLayout`: the exact rows of one file's body with their heights, and the
//! lookups between row keys and pixel offsets.

use polygloss_diff::Side;
use polygloss_diff::rows::{GapId, Row};

use super::anchor::{BlockId, RowKey};
use super::height_index::HeightIndex;
use super::metrics::Metrics;

/// One row of a file body (below the header), as the document sees it. `diff_row`
/// is the index of the row in the file's `polygloss_diff::rows::build_rows`
/// output, for painting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRow {
    /// A code row. Unified context and split rows show both sides; removed and
    /// added rows (and unbalanced split rows) one.
    Line {
        old: Option<u32>,
        new: Option<u32>,
        diff_row: u32,
    },
    /// A hidden run of `len` unchanged lines starting at `old_start` /
    /// `new_start`, with its expander.
    Gap {
        id: GapId,
        old_start: u32,
        new_start: u32,
        len: u32,
        diff_row: u32,
    },
    /// `\ No newline at end of file`.
    NoNewline { side: Side, diff_row: u32 },
    /// Split only: both sides' `\ No newline at end of file` on one row.
    NoNewlineBoth { diff_row: u32 },
    /// A host block below the previous row (T2.7).
    Block(BlockId),
    /// Split only: an old-side and a new-side block hung from the same row,
    /// side by side, each in its own column; the row is as tall as the
    /// taller one.
    BlockPair { old: BlockId, new: BlockId },
    /// A body that is a single message (special files, "Load diff").
    Placeholder,
}

impl BodyRow {
    /// The blocks a block row shows: `[block, None]`, `[old, new]` for a
    /// pair, `[None, None]` for any other row.
    pub fn block_ids(&self) -> [Option<BlockId>; 2] {
        match *self {
            BodyRow::Block(id) => [Some(id), None],
            BodyRow::BlockPair { old, new } => [Some(old), Some(new)],
            _ => [None, None],
        }
    }
}

/// The exact layout of a file body: rows, their heights and the indexes that
/// resolve [`RowKey`]s. Built by the viewport from a materialized file's rows
/// (plus blocks, wrapping and expansions) and handed to
/// [`crate::document::Document::set_file_layout`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileLayout {
    rows: Vec<BodyRow>,
    heights: HeightIndex,
    /// Rows that show an old line or hold an old range (gaps), in order; their
    /// old start lines are strictly increasing.
    old_owners: Vec<u32>,
    /// Same for the new side.
    new_owners: Vec<u32>,
    /// Gap rows, in order.
    gaps: Vec<u32>,
    /// Block rows (single and paired), in order.
    blocks: Vec<u32>,
    /// Split rows: blocks on the two sides of one row pair up
    /// ([`FileLayout::with_blocks`]).
    split: bool,
}

impl FileLayout {
    /// A layout of `rows` with the given heights. Panics if the lengths differ.
    pub fn new(rows: Vec<BodyRow>, heights: &[f32]) -> FileLayout {
        assert_eq!(rows.len(), heights.len(), "one height per row");
        let mut layout = FileLayout {
            heights: HeightIndex::new(heights),
            ..FileLayout::default()
        };
        for (i, row) in rows.iter().enumerate() {
            let i = i as u32;
            if start(row, Side::Old).is_some() {
                layout.old_owners.push(i);
            }
            if start(row, Side::New).is_some() {
                layout.new_owners.push(i);
            }
            match row {
                BodyRow::Gap { .. } => layout.gaps.push(i),
                BodyRow::Block(_) | BodyRow::BlockPair { .. } => layout.blocks.push(i),
                _ => {}
            }
        }
        layout.rows = rows;
        layout
    }

    /// The layout of diff rows at their default heights: lines and markers
    /// `row_height`, gaps `gap_height`.
    pub fn from_rows(rows: &[Row], metrics: &Metrics) -> FileLayout {
        let mut body = Vec::with_capacity(rows.len());
        let mut heights = Vec::with_capacity(rows.len());
        let split = rows
            .iter()
            .any(|r| matches!(r, Row::Split { .. } | Row::NoNewlineBoth));
        for (i, row) in rows.iter().enumerate() {
            let diff_row = i as u32;
            let (body_row, height) = match row {
                Row::Gap { id, old, new, .. } => (
                    BodyRow::Gap {
                        id: *id,
                        old_start: old.start,
                        new_start: new.start,
                        len: old.end - old.start,
                        diff_row,
                    },
                    metrics.gap_height,
                ),
                Row::Unified { old, new, .. } => (
                    BodyRow::Line {
                        old: *old,
                        new: *new,
                        diff_row,
                    },
                    metrics.row_height,
                ),
                Row::Split { left, right } => (
                    BodyRow::Line {
                        old: left.map(|c| c.line),
                        new: right.map(|c| c.line),
                        diff_row,
                    },
                    metrics.row_height,
                ),
                Row::NoNewline { side } => (
                    BodyRow::NoNewline {
                        side: *side,
                        diff_row,
                    },
                    metrics.row_height,
                ),
                Row::NoNewlineBoth => (BodyRow::NoNewlineBoth { diff_row }, metrics.row_height),
            };
            body.push(body_row);
            heights.push(height);
        }
        FileLayout::new(body, &heights).with_split(split)
    }

    /// These rows, marked as split rows (or not): in split, an old-side and a
    /// new-side block on the same row share it ([`BodyRow::BlockPair`]).
    /// [`FileLayout::from_rows`] marks layouts of split rows itself.
    pub fn with_split(mut self, split: bool) -> FileLayout {
        self.split = split;
        self
    }

    /// Whether these are split rows (see [`FileLayout::with_split`]).
    pub fn is_split(&self) -> bool {
        self.split
    }

    /// A body that is one placeholder row of `height`.
    pub fn placeholder(height: f32) -> FileLayout {
        FileLayout::new(vec![BodyRow::Placeholder], &[height])
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> &[BodyRow] {
        &self.rows
    }

    /// Indexes of the block rows, in order.
    pub fn block_rows(&self) -> &[u32] {
        &self.blocks
    }

    /// Total body height.
    pub fn height(&self) -> f64 {
        self.heights.total()
    }

    pub fn row_height(&self, i: usize) -> f32 {
        self.heights.get(i)
    }

    /// Top of row `i` relative to the body top.
    pub fn row_top(&self, i: usize) -> f64 {
        self.heights.prefix(i)
    }

    /// The row containing body offset `y` and the offset within it
    /// ([`HeightIndex::find`]).
    pub fn row_at(&self, y: f64) -> (usize, f64) {
        self.heights.find(y)
    }

    /// Changes one row's height (a measured block, a wrapped line).
    pub fn set_row_height(&mut self, i: usize, h: f32) {
        self.heights.set(i, h);
    }

    /// The row a key resolves to. `Line` resolves to the row showing that
    /// line, the gap hiding it, or else the nearest earlier row of that side;
    /// `Gap` to the first run of that gap; `Header` never (it is not a row).
    pub fn find(&self, key: RowKey) -> Option<usize> {
        let row =
            match key {
                RowKey::Header => None,
                RowKey::Line { side, line } => {
                    let owners = match side {
                        Side::Old => &self.old_owners,
                        Side::New => &self.new_owners,
                    };
                    // Owners' start lines strictly increase: the last one starting
                    // at or before `line` shows it, hides it in a gap, or is the
                    // nearest earlier row of that side.
                    let after = owners.partition_point(|&r| {
                        start(&self.rows[r as usize], side).is_some_and(|s| s <= line)
                    });
                    after.checked_sub(1).map(|i| owners[i])
                }
                RowKey::Gap(gap) => self.gaps.iter().copied().find(
                    |&r| matches!(self.rows[r as usize], BodyRow::Gap { id, .. } if id == gap),
                ),
                RowKey::Block(block) => {
                    self.blocks
                        .iter()
                        .copied()
                        .find(|&r| match self.rows[r as usize] {
                            BodyRow::Block(id) => id == block,
                            BodyRow::BlockPair { old, new } => old == block || new == block,
                            _ => false,
                        })
                }
                RowKey::Placeholder => self
                    .rows
                    .iter()
                    .position(|r| *r == BodyRow::Placeholder)
                    .map(|r| r as u32),
            };
        row.map(|r| r as usize)
    }

    /// The key of the row at body offset `y`, and `y`'s offset from that
    /// row's top. Rows without a key of their own (`\ No newline` markers) use
    /// the nearest earlier keyed row. `None` for an empty layout or when no
    /// row at or above `y` has a key.
    pub fn key_at(&self, y: f64) -> Option<(RowKey, f64)> {
        if self.rows.is_empty() {
            return None;
        }
        let (at, _) = self.heights.find(y);
        (0..=at).rev().find_map(|i| {
            let key = match self.rows[i] {
                // New first: the side a review is about, and the same in both layouts.
                BodyRow::Line {
                    new: Some(line), ..
                } => RowKey::Line {
                    side: Side::New,
                    line,
                },
                BodyRow::Line {
                    old: Some(line), ..
                } => RowKey::Line {
                    side: Side::Old,
                    line,
                },
                // A gap run is keyed by its first old line: unique per run
                // (unlike its `GapId`) and still valid once the run is revealed.
                BodyRow::Gap { old_start, .. } => RowKey::Line {
                    side: Side::Old,
                    line: old_start,
                },
                BodyRow::Block(id) => RowKey::Block(id),
                // Either id resolves to the row; the old side's comes first.
                BodyRow::BlockPair { old, .. } => RowKey::Block(old),
                BodyRow::Placeholder => RowKey::Placeholder,
                BodyRow::Line {
                    old: None,
                    new: None,
                    ..
                }
                | BodyRow::NoNewline { .. }
                | BodyRow::NoNewlineBoth { .. } => return None,
            };
            Some((key, y - self.heights.prefix(i)))
        })
    }

    /// Bytes owned on the heap.
    pub fn heap_bytes(&self) -> usize {
        let indexes = self.old_owners.capacity()
            + self.new_owners.capacity()
            + self.gaps.capacity()
            + self.blocks.capacity();
        self.rows.capacity() * size_of::<BodyRow>()
            + self.heights.heap_bytes()
            + indexes * size_of::<u32>()
    }
}

/// The first line of `side` that `row` shows or hides, if any.
fn start(row: &BodyRow, side: Side) -> Option<u32> {
    match (row, side) {
        (BodyRow::Line { old, .. }, Side::Old) => *old,
        (BodyRow::Line { new, .. }, Side::New) => *new,
        (BodyRow::Gap { old_start, .. }, Side::Old) => Some(*old_start),
        (BodyRow::Gap { new_start, .. }, Side::New) => Some(*new_start),
        _ => None,
    }
}
