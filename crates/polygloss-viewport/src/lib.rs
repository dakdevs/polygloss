//! Polygloss diff viewport: document model and GPUI view/element. Knows nothing
//! about SQLite, git or review semantics (threads are opaque blocks).
//!
//! - [`document`]: every file's height, the logical scroll anchor, the
//!   materialization window and eviction (pure Rust), with files in display
//!   order and hidden files skipped ([`SlotRange`], [`Document::shown_files`]):
//!   every API stays keyed by `file_idx`.
//! - [`DiffViewport`]: the GPUI view. It reads files and blobs from a
//!   [`DiffProvider`] and paints each file as a card on the canvas
//!   ([`CardStyle`]; the host's header card above the first one,
//!   [`DiffViewport::set_prelude`]) and only visible rows, split or unified
//!   (an added or deleted file is one full-width pane in both, [`one_sided`]),
//!   with tinted line numbers, change bars, word highlights and syntax
//!   colors; sticky file headers with a collapse chevron, the path, kind
//!   pills, the host's review-state pills ([`FileFlags`]), open in editor,
//!   the `+a −d` pill ([`group_digits`]), a Viewed pill and a ⋯ menu; gap
//!   expanders; and placeholders for binary, submodule, generated and large
//!   files.
//! - [`pipeline`]: the prioritized, cancellable background work behind it:
//!   loads and highlights for the files near the viewport, then sizes and
//!   line counts of every file.
//! - [`blocks`]: host elements (threads, composers, notes) below their
//!   anchored line, measured near the viewport, with split spacers.
//! - [`find`]: the host's find matches marked in the code (⌘F).
//! - [`motion`]: the motion core the app shares (ADR-0030): the policy,
//!   the tokens and [`motion::Track`], the sampler every motion runs on.
//! - Category sections ([`Section`], [`DiffViewport::set_sections`]): the
//!   host's sections after the other files, each behind a band on the
//!   canvas that opens or closes it; closed sections cost nothing per
//!   frame.
//! - The line cursor and ranges ([`CursorPos`], `j`/`k`, `⇧↑`/`⇧↓`, `]`/`[`,
//!   `n`/`p`, `e`/`E`, `c`), the "+" on hovered line numbers, dragging
//!   across them for a range, and text selection with copy.
//!
//! The ⋯ menu is a gpui-kit `PopupMenu`: hosts initialize gpui-kit first, as
//! every Polygloss window does, with [`kit::init_kit`] (`gpui_kit::init`
//! without its startup font scan).

pub mod blocks;
mod card;
mod controls;
mod cursor;
pub mod debug;
pub mod document;
mod element;
mod file_flags;
pub mod find;
pub mod gap;
mod gutter;
mod header;
pub mod kit;
pub mod layout;
pub mod materialize;
pub mod motion;
pub mod numbers;
mod paint_rows;
pub mod pipeline;
pub mod provider;
mod reveal;
mod section_band;
mod selection;
pub mod space;
pub mod special;
pub mod style;
mod text_cache;
mod title;
mod view;

pub use blocks::{BlockSpec, ESTIMATED_BLOCK_ROWS, RenderBlock};
pub use card::CardStyle;
pub use controls::ControlAction;
pub use cursor::{CursorPos, Direction};
pub use debug::{
    BandDebug, CardDebug, ControlDebug, HeaderDebug, IconDebug, MenuDebug, PlusDebug, RevealDebug,
    TitleStyle, ViewportDebug,
};
pub use document::{
    BlockAnchor, BlockId, BodyRow, Document, FileLayout, FileState, HeightIndex, Metrics,
    PlacedBlock, RowKey, ScrollAnchor, SectionFiles, ShownFiles, SizeHint, SlotRange,
};
pub use file_flags::FileFlags;
pub use find::{FindCurrent, FindHighlights, FindMatcher};
pub use layout::{LayoutMode, resolve_layout};
pub use materialize::{LoadError, LoadOptions, Loaded, MaterializedFile};
pub use numbers::{group_digits, one_sided};
pub use pipeline::{FileCounts, PipelineStats};
pub use provider::DiffProvider;
pub use section_band::{BandFlags, Section, SectionCounts};
pub use style::{DiffStyle, Indicators, ViewportTheme};
pub use view::{
    DiffViewport, FrameStats, ScrollTarget, ViewportEvent, ViewportOptions, code_font,
    code_font_features,
};
