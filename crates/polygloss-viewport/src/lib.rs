//! Polygloss diff viewport: document model and GPUI view/element. Knows nothing
//! about SQLite, git or review semantics (threads are opaque blocks).

pub mod document;
pub mod materialize;

pub use document::{
    BlockId, BodyRow, Document, FileLayout, FileState, HeightIndex, Metrics, RowKey, ScrollAnchor,
    SizeHint,
};
pub use materialize::MaterializedFile;
