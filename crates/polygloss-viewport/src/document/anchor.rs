//! Logical scroll positions: row keys and the scroll anchor (design §12.4).
//!
//! The scroll position is never a pixel offset. It is a [`ScrollAnchor`]: a
//! file, a row key inside it and a pixel offset from that row's top. Pixels are
//! derived from it whenever heights change, so a height correction above the
//! anchor (an estimate becoming exact, a block growing) never moves the
//! content on screen.

use polygloss_diff::Side;
use polygloss_diff::rows::GapId;

/// A host-provided variable-height block (a thread, composer or note; T2.7).
/// Opaque to the viewport: the host picks the ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u64);

/// Identifies a row of a file independently of layout (split or unified),
/// expansions and heights, so it survives every relayout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKey {
    /// The file header (the top of the file).
    Header,
    /// The row showing `line` (0-based) of `side`. A line hidden in a gap
    /// resolves to that gap's row; a split row matches either of its lines.
    Line { side: Side, line: u32 },
    /// The expander row of gap `GapId`. When restored or revealed ranges split
    /// a gap into several hidden runs, this resolves to the first run; anchors
    /// derived from a scroll position use the run's first line instead
    /// ([`RowKey::Line`] with [`Side::Old`]), which is unique per run.
    Gap(GapId),
    /// A block row (T2.7).
    Block(BlockId),
    /// A body that is not rows yet or never will be: a file still estimated or
    /// loading, a special file (binary, submodule), "Load diff".
    Placeholder,
}

/// The scroll position: the viewport's top edge is `offset_px` below the top
/// of row `row` of file `file_idx`. `offset_px` is usually within the row, but
/// may be larger or negative (see [`crate::document::Document::scroll_by`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollAnchor {
    pub file_idx: u32,
    pub row: RowKey,
    pub offset_px: f32,
}

impl Default for ScrollAnchor {
    /// The top of the first file.
    fn default() -> ScrollAnchor {
        ScrollAnchor {
            file_idx: 0,
            row: RowKey::Header,
            offset_px: 0.0,
        }
    }
}
