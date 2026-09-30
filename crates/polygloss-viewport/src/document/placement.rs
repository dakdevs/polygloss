//! Where host blocks go in a file body (design §11.6 "Threads", §12.4
//! "Blocks"): below their anchored line, in the host's order, as rows of
//! their own. Pure placement; the viewport renders and measures them.

use polygloss_diff::Side;

use super::anchor::{BlockId, RowKey};
use super::file_layout::{BodyRow, FileLayout};

/// What a block hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockAnchor {
    /// Below the row showing `line` (0-based) of `side`: for a range, its last
    /// line. A line hidden in a gap puts the block below that gap; a line past
    /// the end of its side, below the side's last row; a side the file does
    /// not have (the old side of an added file), at the end of the body. In
    /// split, the block sits in `side`'s column.
    Line { side: Side, line: u32 },
    /// Right under the file header, full width (a file-level thread or
    /// composer).
    FileTop,
}

/// A block as the document lays it out: its id, anchor and current height
/// (estimated until the viewport measures it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedBlock {
    pub id: BlockId,
    pub anchor: BlockAnchor,
    pub height: f32,
}

impl FileLayout {
    /// These rows with `blocks` placed in them, replacing any blocks placed
    /// before. Rows keep their heights. A block goes below its anchor row and
    /// below the `\ No newline` markers right after it (they belong to that
    /// line); blocks with the same place keep the order of `blocks`. In split
    /// rows ([`FileLayout::is_split`]), the old-side and new-side line blocks
    /// of one place pair up in that order, the first old one with the first
    /// new one and so on: each pair shares a row ([`BodyRow::BlockPair`], as
    /// tall as the taller block) where the earlier of the two would go, and
    /// the blocks left over get rows of their own.
    pub fn with_blocks(&self, blocks: &[PlacedBlock]) -> FileLayout {
        let rows = self.rows();
        let block_rows = self.block_rows();
        // Rows (without blocks) that come before full-layout row `i`.
        let base_before = |i: usize| i - block_rows.partition_point(|&r| (r as usize) < i);
        let base_len = base_before(rows.len());
        // `(place, after the file-top blocks, host order)`: a line block that
        // lands at the very top (an empty body) still goes below every
        // file-top block.
        let mut places: Vec<(usize, bool, usize)> = blocks
            .iter()
            .enumerate()
            .map(|(k, block)| {
                let place = match block.anchor {
                    BlockAnchor::FileTop => 0,
                    BlockAnchor::Line { side, line } => {
                        match self.find(RowKey::Line { side, line }) {
                            Some(row) => {
                                let mut end = row + 1;
                                while end < rows.len()
                                    && matches!(
                                        rows[end],
                                        BodyRow::NoNewline { .. }
                                            | BodyRow::NoNewlineBoth { .. }
                                            | BodyRow::Block(_)
                                            | BodyRow::BlockPair { .. }
                                    )
                                {
                                    end += 1;
                                }
                                base_before(end)
                            }
                            None => base_len,
                        }
                    }
                };
                (place, block.anchor != BlockAnchor::FileTop, k)
            })
            .collect();
        places.sort_unstable();

        let len = base_len + blocks.len();
        let mut out_rows = Vec::with_capacity(len);
        let mut heights = Vec::with_capacity(len);
        let mut group = Vec::new();
        let mut at = 0;
        let mut base = 0;
        let mut place_rows = |place: usize, out: &mut Vec<BodyRow>, h: &mut Vec<f32>| {
            group.clear();
            while at < places.len() && places[at].0 == place {
                group.push(places[at].2);
                at += 1;
            }
            push_group(&group, blocks, self.is_split(), out, h);
        };
        for (i, row) in rows.iter().enumerate() {
            if let BodyRow::Block(_) | BodyRow::BlockPair { .. } = row {
                continue;
            }
            place_rows(base, &mut out_rows, &mut heights);
            out_rows.push(*row);
            heights.push(self.row_height(i));
            base += 1;
        }
        place_rows(base, &mut out_rows, &mut heights);
        debug_assert_eq!(at, places.len(), "every block is placed");
        FileLayout::new(out_rows, &heights).with_split(self.is_split())
    }
}

/// The side of a block anchored to a line.
fn line_side(block: &PlacedBlock) -> Option<Side> {
    match block.anchor {
        BlockAnchor::Line { side, .. } => Some(side),
        BlockAnchor::FileTop => None,
    }
}

/// Appends the rows of the blocks `group` (indexes into `blocks`, in order)
/// that share one place: one row each, except that in `split` the `i`-th
/// old-side and `i`-th new-side line blocks share a row.
fn push_group(
    group: &[usize],
    blocks: &[PlacedBlock],
    split: bool,
    rows: &mut Vec<BodyRow>,
    heights: &mut Vec<f32>,
) {
    // `partner[j]`: the index in `group` of the block `group[j]` pairs with.
    let mut partner = vec![None; group.len()];
    if split {
        let side_of = |side| {
            (0..group.len())
                .filter(move |&j| line_side(&blocks[group[j]]) == Some(side))
                .collect::<Vec<_>>()
        };
        for (o, n) in side_of(Side::Old).into_iter().zip(side_of(Side::New)) {
            partner[o] = Some(n);
            partner[n] = Some(o);
        }
    }
    let mut done = vec![false; group.len()];
    for j in 0..group.len() {
        if done[j] {
            continue;
        }
        let block = &blocks[group[j]];
        match partner[j] {
            Some(p) => {
                done[p] = true;
                let other = &blocks[group[p]];
                let (old, new) = match line_side(block) {
                    Some(Side::Old) => (block, other),
                    _ => (other, block),
                };
                rows.push(BodyRow::BlockPair {
                    old: old.id,
                    new: new.id,
                });
                heights.push(block_height(old.height).max(block_height(new.height)));
            }
            None => {
                rows.push(BodyRow::Block(block.id));
                heights.push(block_height(block.height));
            }
        }
    }
}

/// A usable row height: negative, NaN or infinite heights count as 0.
pub(crate) fn block_height(h: f32) -> f32 {
    if h.is_finite() { h.max(0.0) } else { 0.0 }
}
