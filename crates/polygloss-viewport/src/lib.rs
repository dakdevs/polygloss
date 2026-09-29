//! Polygloss diff viewport: document model and GPUI view/element. Knows nothing
//! about SQLite, git or review semantics (threads are opaque blocks).
//!
//! - [`document`]: every file's height, the logical scroll anchor, the
//!   materialization window and eviction (pure Rust).
//! - [`DiffViewport`]: the GPUI view. It reads files and blobs from a
//!   [`DiffProvider`], materializes files near the viewport in the background
//!   and paints only visible rows, split or unified, with line numbers, change
//!   indicators, word highlights and syntax colors; sticky file headers with
//!   the host's review flags ([`FileFlags`]), a collapse chevron, a Viewed
//!   checkbox and a ⋯ menu; gap expanders; and placeholders for binary,
//!   submodule, generated and large files.
//!
//! The ⋯ menu is a gpui-kit `PopupMenu`: hosts call `gpui_kit::init` first,
//! as every Polygloss window does.

mod controls;
pub mod debug;
pub mod document;
mod element;
mod file_flags;
pub mod gap;
mod gutter;
mod header;
pub mod layout;
pub mod materialize;
mod paint_rows;
pub mod provider;
pub mod special;
pub mod style;
mod text_cache;
mod view;

pub use controls::ControlAction;
pub use debug::{ControlDebug, HeaderDebug, MenuDebug, ViewportDebug};
pub use document::{
    BlockId, BodyRow, Document, FileLayout, FileState, HeightIndex, Metrics, RowKey, ScrollAnchor,
    SizeHint,
};
pub use file_flags::FileFlags;
pub use layout::{LayoutMode, resolve_layout};
pub use materialize::MaterializedFile;
pub use provider::DiffProvider;
pub use style::{DiffStyle, Indicators, ViewportTheme};
pub use view::{DiffViewport, FrameStats, ScrollTarget, ViewportEvent, ViewportOptions};
