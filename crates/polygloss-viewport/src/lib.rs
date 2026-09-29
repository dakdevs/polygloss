//! Polygloss diff viewport: document model and GPUI view/element. Knows nothing
//! about SQLite, git or review semantics (threads are opaque blocks).
//!
//! - [`document`]: every file's height, the logical scroll anchor, the
//!   materialization window and eviction (pure Rust).
//! - [`DiffViewport`]: the GPUI view. It reads files and blobs from a
//!   [`DiffProvider`], materializes files near the viewport in the background
//!   and paints only visible rows, split or unified, with line numbers, change
//!   indicators, word highlights and syntax colors.

pub mod debug;
pub mod document;
mod element;
mod gutter;
pub mod layout;
pub mod materialize;
mod paint_rows;
pub mod provider;
pub mod style;
mod text_cache;
mod view;

pub use debug::ViewportDebug;
pub use document::{
    BlockId, BodyRow, Document, FileLayout, FileState, HeightIndex, Metrics, RowKey, ScrollAnchor,
    SizeHint,
};
pub use layout::{LayoutMode, resolve_layout};
pub use materialize::MaterializedFile;
pub use provider::DiffProvider;
pub use style::{DiffStyle, Indicators, ViewportTheme};
pub use view::{DiffViewport, FrameStats, ScrollTarget, ViewportEvent, ViewportOptions};
