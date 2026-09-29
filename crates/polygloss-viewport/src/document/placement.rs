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
    /// line); blocks with the same place keep the order of `blocks`.
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
        let mut next = places.iter().peekable();
        let mut base = 0;
        for (i, row) in rows.iter().enumerate() {
            if let BodyRow::Block(_) = row {
                continue;
            }
            while let Some(&(_, _, k)) = next.next_if(|(place, _, _)| *place == base) {
                out_rows.push(BodyRow::Block(blocks[k].id));
                heights.push(block_height(blocks[k].height));
            }
            out_rows.push(*row);
            heights.push(self.row_height(i));
            base += 1;
        }
        for &(_, _, k) in next {
            out_rows.push(BodyRow::Block(blocks[k].id));
            heights.push(block_height(blocks[k].height));
        }
        FileLayout::new(out_rows, &heights)
    }
}

/// A usable row height: negative, NaN or infinite heights count as 0.
pub(crate) fn block_height(h: f32) -> f32 {
    if h.is_finite() { h.max(0.0) } else { 0.0 }
}
