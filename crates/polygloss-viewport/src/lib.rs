//! Polygloss diff viewport: document model and GPUI view/element. Knows nothing
//! about SQLite, git or review semantics (threads are opaque blocks).
//!
//! - [`document`]: every file's height, the logical scroll anchor, the
//!   materialization window and eviction (pure Rust).
//! - [`DiffViewport`]: the GPUI view. It reads files and blobs from a
//!   [`DiffProvider`] and paints only visible rows, split or unified, with
//!   line numbers, change indicators, word highlights and syntax colors;
//!   sticky file headers with the host's review flags ([`FileFlags`]), a
//!   collapse chevron, a Viewed checkbox and a ⋯ menu; gap expanders; and
//!   placeholders for binary, submodule, generated and large files.
//! - [`pipeline`]: the prioritized, cancellable background work behind it:
//!   loads and highlights for the files near the viewport, then sizes and
//!   line counts of every file.
//! - [`blocks`]: host elements (threads, composers, notes) below their
//!   anchored line, measured near the viewport, with split spacers.
//! - The line cursor and ranges ([`CursorPos`], `j`/`k`, `⇧↑`/`⇧↓`, `]`/`[`,
//!   `n`/`p`, `e`/`E`, `c`), the "+" on hovered line numbers, dragging
//!   across them for a range, and text selection with copy.
//!
//! The ⋯ menu is a gpui-kit `PopupMenu`: hosts initialize gpui-kit first, as
//! every Polygloss window does, with [`kit::init_kit`] (`gpui_kit::init`
//! without its startup font scan).

pub mod blocks;
mod controls;
mod cursor;
pub mod debug;
pub mod document;
mod element;
mod file_flags;
pub mod gap;
mod gutter;
mod header;
pub mod kit;
pub mod layout;
pub mod materialize;
mod paint_rows;
pub mod pipeline;
pub mod provider;
mod selection;
pub mod special;
pub mod style;
mod text_cache;
mod view;

pub use blocks::{BlockSpec, ESTIMATED_BLOCK_ROWS, RenderBlock};
pub use controls::ControlAction;
pub use cursor::{CursorPos, Direction};
pub use debug::{ControlDebug, HeaderDebug, MenuDebug, PlusDebug, ViewportDebug};
pub use document::{
    BlockAnchor, BlockId, BodyRow, Document, FileLayout, FileState, HeightIndex, Metrics,
    PlacedBlock, RowKey, ScrollAnchor, SizeHint,
};
pub use file_flags::FileFlags;
pub use layout::{LayoutMode, resolve_layout};
pub use materialize::{LoadError, LoadOptions, Loaded, MaterializedFile};
pub use pipeline::{FileCounts, PipelineStats};
pub use provider::DiffProvider;
pub use style::{DiffStyle, Indicators, ViewportTheme};
pub use view::{DiffViewport, FrameStats, ScrollTarget, ViewportEvent, ViewportOptions};
