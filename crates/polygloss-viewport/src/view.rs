//! `DiffViewport`: the GPUI view over one diff (design §11.6, §12.4).
//!
//! It owns the [`Document`] (every file's height and the logical scroll
//! anchor), the shaped-line cache and the [`Pipeline`] that turns files near
//! the viewport into [`MaterializedFile`]s in the background: blobs → diff →
//! word ranges → rows (swapped in, laid out) → syntax tokens (swapped in
//! without moving anything), prioritized and cancellable (see
//! [`crate::pipeline`]). The view only ever sees results through the
//! document's generation-checked states.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    Bounds, Context, EventEmitter, Font, FontFeatures, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Window, div, font, px,
};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::Layout;
use polygloss_diff::word::Granularity;
use polygloss_diff::{FileChange, FileKind, Side};

use crate::blocks::Blocks;
use crate::card::{CardStyle, Prelude, inner_bounds, inner_span};
use crate::controls::Pressed;
use crate::cursor::Cursor;
use crate::document::{
    BlockId, DEFAULT_EVICTION_BUDGET_BYTES, DEFAULT_WINDOW_SCREENS, Document, FileLayout,
    FileState, RowKey, ScrollAnchor,
};
use crate::element::DiffElement;
use crate::file_flags::FileFlags;
use crate::find::{FindCurrent, FindHighlights, FindState};
use crate::gap::Gaps;
use crate::header::HeaderMenu;
use crate::layout::{
    Columns, DEFAULT_CODE_FONT_SIZE, Geometry, LayoutMode, Pane, digits, layout_for,
    resolve_layout, wrapped_heights,
};
use crate::materialize::MaterializedFile;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::DebugRow;
use crate::paint_rows::{Frame, Marks, Painter, failed_label};
use crate::pipeline::{Applied, Done, FileCounts, Pipeline, PipelineStats};
use crate::provider::DiffProvider;
use crate::reveal::{Reveal, RevealGeom};
use crate::section_band::Band;
use crate::selection::{Drag, TextSelection};
use crate::special::{BodyLabel, Specials, large_label, needs_blobs};
use crate::style::{DiffStyle, ViewportTheme};
use crate::text_cache::{TEXT_CACHE_CAPACITY, TextCache};

/// Times a frame is built at most while wrapped rows are measured (each pass
/// fixes the heights of the rows the previous one found mis-estimated).
const WRAP_PASSES: usize = 3;

/// Everything that configures a viewport. Changing it with
/// [`DiffViewport::set_options`] keeps the scroll anchor.
#[derive(Debug, Clone)]
pub struct ViewportOptions {
    pub layout: LayoutMode,
    /// Auto layout is split from this many code-font columns (design §18:
    /// 160), with ±8 columns of hysteresis.
    pub split_min_columns: u32,
    /// Word or char highlights on paired lines; `None` turns them off.
    pub word_diff: Option<Granularity>,
    pub diff: DiffOptions,
    pub style: DiffStyle,
    /// Code font family (design §18: Lilex; "SF Mono" and "System Mono" name
    /// the system monospaced font, which also replaces a missing family).
    pub code_font: SharedString,
    pub code_font_size: f32,
    /// OpenType ligatures in the code font (settings `buffer_font.ligatures`,
    /// default off): off, `->` shows as the two characters it is.
    pub ligatures: bool,
    /// The UI font: header pills and the Viewed label (design §11.10: the
    /// system font).
    pub ui_font: Font,
    pub theme: Arc<ViewportTheme>,
    /// Files with more changed lines show a placeholder instead of rows
    /// (design §12.3: 20,000).
    pub large_file_changed_lines: u32,
    /// Syntax highlighting. Plain text paints first either way; tokens swap
    /// in when they are ready.
    pub syntax: bool,
    /// Files within this many screens above and below the viewport are
    /// materialized (design §12.4: 2, **Provisional**).
    pub window_screens: f32,
    /// Materialized data above this many bytes is evicted, farthest file
    /// first (design §12.4: 256 MiB, **Provisional**).
    pub eviction_budget_bytes: usize,
    /// Each file as a card on the canvas (design §11.6); `None` is the flat
    /// v1 layout.
    pub cards: Option<CardStyle>,
}

impl Default for ViewportOptions {
    fn default() -> ViewportOptions {
        ViewportOptions {
            layout: LayoutMode::Auto,
            split_min_columns: 160,
            word_diff: Some(Granularity::Word),
            diff: DiffOptions::default(),
            style: DiffStyle::default(),
            code_font: SharedString::new_static("Lilex"),
            code_font_size: DEFAULT_CODE_FONT_SIZE,
            ligatures: false,
            ui_font: font(".AppleSystemUIFont"),
            theme: Arc::new(ViewportTheme::default()),
            large_file_changed_lines: 20_000,
            syntax: true,
            window_screens: DEFAULT_WINDOW_SCREENS,
            eviction_budget_bytes: DEFAULT_EVICTION_BUDGET_BYTES,
            cards: Some(CardStyle::default()),
        }
    }
}

/// What [`DiffViewport::scroll_to`] brings to the top of what is visible.
/// Every target but [`ScrollTarget::Restore`] is explicit: a target in a
/// closed section opens it first ([`ViewportEvent::SectionToggled`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    /// A file's header, at the viewport's top edge.
    File(u32),
    /// The row showing `line` (0-based) of `side`, or the gap hiding it,
    /// right below its file's (pinned) header.
    Line {
        file_idx: u32,
        side: Side,
        line: u32,
    },
    /// A host block (T2.7), right below its file's (pinned) header.
    Block(BlockId),
    /// A [`ScrollTarget::Line`] restored from view state: it never opens a
    /// section; in a closed one it lands on the section's band.
    Restore {
        file_idx: u32,
        side: Side,
        line: u32,
    },
}

/// Timing of one frame (plan T2.9 `scroll_p95_ms`: prepaint + paint CPU time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    pub prepaint: Duration,
    pub paint: Duration,
    /// Rows painted, headers included.
    pub visible_rows: u32,
    /// Lines shaped this frame (shaped-line cache misses).
    pub shaped_lines: u32,
    /// Visible rows whose file is still loading (plan T2.9 `first_paint_ms`
    /// waits for 0).
    pub loading_rows: u32,
    /// Visible code rows painted as plain text while their tokens are being
    /// computed, plus loading rows (plan T2.9 `highlight_ms` waits for 0).
    pub unhighlighted_rows: u32,
}

/// What the viewport reports to its host. The viewport renders and emits;
/// the host owns review state (design §11.6).
#[derive(Debug, Clone, PartialEq)]
pub enum ViewportEvent {
    /// The file at the top of the viewport changed (scrolling or a jump).
    VisibleFileChanged(u32),
    CursorMoved {
        file_idx: u32,
        side: Side,
        line: u32,
    },
    CommentRequested {
        file_idx: u32,
        side: Side,
        start_line: u32,
        line: u32,
    },
    FileCommentRequested(u32),
    ViewedToggled(u32),
    OpenInEditor {
        file_idx: u32,
        side: Side,
        line: u32,
    },
    LoadDiffRequested(u32),
    /// A file listed as text turned out binary when its blobs were first read
    /// (a NUL byte, git's rule): the viewport now shows it as binary, and the
    /// host may store the kind (`file_changes.kind`).
    BinaryDetected(u32),
    /// Emitted after every painted frame.
    FrameStats(FrameStats),
    /// Line counts landed ([`DiffViewport::file_counts`]): at most once per
    /// pipeline batch that brought new counts, never per frame.
    CountsUpdated,
    /// The viewport opened or closed a section: its band's Show / Hide, or
    /// an explicit target in it. Never for the host's own calls.
    SectionToggled {
        id: u32,
        open: bool,
    },
    /// A band's Mark all viewed (or unviewed) was clicked; the host marks the
    /// section's files ([`DiffViewport::set_band_all_viewed`]).
    SectionMarkViewed(u32),
}

/// Which layout a file's rows were built for; a different key means the file
/// must be laid out again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LayoutKey {
    layout: Layout,
    /// Code column width in px when wrapping, 0 when not.
    wrap: u32,
    /// Changed lines above which the file shows "Load diff" (`u32::MAX` once
    /// the user asked to load it).
    large_file_changed_lines: u32,
    /// The file's revealed-context version (T2.5 gaps).
    expansions: u64,
    load_requested: bool,
}

/// The GPUI view over one diff.
pub struct DiffViewport {
    pub(crate) provider: Arc<dyn DiffProvider>,
    pub(crate) opts: ViewportOptions,
    pub(crate) files: Arc<Vec<FileChange>>,
    pub(crate) doc: Document,
    pub(crate) geometry: Geometry,
    pub(crate) code_font: Font,
    pub(crate) geometry_dirty: bool,
    /// The effective layout; `measured` once a frame has seen the width.
    pub(crate) layout: Layout,
    pub(crate) measured: bool,
    /// The rows' width: a card's inner width (the viewport's in the flat
    /// layout), which the layout, wrapping and blocks go by.
    pub(crate) width: f32,
    /// The viewport's width.
    pub(crate) outer_width: f32,
    /// The host's prelude ([`DiffViewport::set_prelude`]).
    pub(crate) prelude: Option<Prelude>,
    layout_keys: Vec<Option<LayoutKey>>,
    /// What a file's body shows when it has no code rows (special files,
    /// large diffs, load errors).
    pub(crate) labels: Vec<Option<BodyLabel>>,
    pub(crate) text_cache: TextCache,
    pub(crate) pipeline: Pipeline,
    pub(crate) frame_pool: Option<Frame>,
    pub(crate) top_file: u32,
    /// Host-owned review state per file (headers).
    pub(crate) flags: Vec<FileFlags>,
    /// Revealed context per file.
    pub(crate) gaps: Gaps,
    /// LFS pointers, "Load diff" requests.
    pub(crate) special: Specials,
    /// The open ⋯ menu.
    pub(crate) menu: Option<HeaderMenu>,
    /// A ⋯ menu asked for from the keyboard while its header was off
    /// screen: it opens once the header is painted.
    pub(crate) pending_menu: Option<u32>,
    /// The control a mouse button went down on (a click needs the release
    /// there too).
    pub(crate) pressed: Option<Pressed>,
    /// Host blocks (threads, composers, notes; see [`crate::blocks`]).
    pub(crate) blocks: Blocks,
    /// What each section's band shows ([`crate::section_band`]), in the
    /// document's section order.
    pub(crate) bands: Vec<Band>,
    /// The line cursor and range ([`crate::cursor`]).
    pub(crate) cursor: Cursor,
    /// Selected text ([`crate::selection`]).
    pub(crate) selection: Option<TextSelection>,
    /// A mouse drag across line numbers or code.
    pub(crate) drag: Option<Drag>,
    /// The pointer is over the viewport (not a header or a popup over it).
    pub(crate) pointer_inside: bool,
    /// Old-side lines may be commented on (the host turns it off, e.g. for
    /// "Changes since last review", OQ-9).
    pub(crate) old_side_comments: bool,
    /// Find matches marked in the code ([`crate::find`]).
    pub(crate) find: Option<FindState>,
    /// The running reveal ([`crate::reveal`]).
    pub(crate) reveal: Option<Reveal>,
    /// [`DiffViewport::hold_layout`]: the layout, wrap and measurement
    /// widths stay at the held width instead of following the live one.
    held: bool,
    #[cfg(feature = "debug-inspect")]
    relayouts: u32,
    #[cfg(feature = "debug-inspect")]
    pub(crate) debug_rows: Vec<DebugRow>,
    #[cfg(feature = "debug-inspect")]
    pub(crate) debug_headers: Vec<crate::debug::HeaderDebug>,
    #[cfg(feature = "debug-inspect")]
    pub(crate) debug_bands: Vec<crate::debug::BandDebug>,
    #[cfg(feature = "debug-inspect")]
    pub(crate) debug_text: Vec<(f32, f32, std::rc::Rc<crate::text_cache::ShapedText>)>,
}

impl EventEmitter<ViewportEvent> for DiffViewport {}

impl DiffViewport {
    pub fn new(
        provider: Arc<dyn DiffProvider>,
        opts: ViewportOptions,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> DiffViewport {
        let files = provider.files();
        let (code_font, geometry) = resolve_font(&opts, window);
        // A first guess from the window; the first frame decides with the
        // viewport's real width.
        let outer_width = window.viewport_size().width.as_f32();
        let (_, width) = inner_span(outer_width, opts.cards);
        let layout = resolve_layout(
            opts.layout,
            width / geometry.advance,
            opts.split_min_columns,
            None,
        );
        let doc = Document::new(
            files.clone(),
            geometry.metrics(layout, opts.large_file_changed_lines, opts.cards),
        );
        let special = Specials::default();
        let file_count = files.len();
        let labels = files
            .iter()
            .enumerate()
            .map(|(f, c)| special.body_label(f as u32, c, None))
            .collect();
        DiffViewport {
            pipeline: Pipeline::new(provider.clone(), files.clone(), opts.theme.syntax.clone()),
            layout_keys: vec![None; files.len()],
            provider,
            opts,
            files,
            doc,
            geometry,
            code_font,
            geometry_dirty: false,
            layout,
            measured: false,
            width,
            outer_width,
            prelude: None,
            labels,
            text_cache: TextCache::new(TEXT_CACHE_CAPACITY),
            frame_pool: None,
            top_file: 0,
            flags: vec![FileFlags::default(); file_count],
            gaps: Gaps::default(),
            special,
            menu: None,
            pending_menu: None,
            pressed: None,
            blocks: Blocks::default(),
            bands: Vec::new(),
            cursor: Cursor::default(),
            selection: None,
            drag: None,
            pointer_inside: false,
            old_side_comments: true,
            find: None,
            reveal: None,
            held: false,
            #[cfg(feature = "debug-inspect")]
            relayouts: 0,
            #[cfg(feature = "debug-inspect")]
            debug_rows: Vec::new(),
            #[cfg(feature = "debug-inspect")]
            debug_headers: Vec::new(),
            #[cfg(feature = "debug-inspect")]
            debug_bands: Vec::new(),
            #[cfg(feature = "debug-inspect")]
            debug_text: Vec::new(),
        }
    }

    pub fn options(&self) -> &ViewportOptions {
        &self.opts
    }

    /// Marks every match of `highlights.matcher` in the code painted, the
    /// current one emphasized (⌘F); `None` clears the marks.
    pub fn set_find_highlights(
        &mut self,
        highlights: Option<FindHighlights>,
        cx: &mut Context<Self>,
    ) {
        self.find = highlights.map(FindState::new);
        cx.notify();
    }

    /// Emphasizes another match, keeping the matcher (and the lines it
    /// already matched). Does nothing without highlights.
    pub fn set_find_current(&mut self, current: Option<FindCurrent>, cx: &mut Context<Self>) {
        if let Some(find) = &mut self.find
            && find.highlights.current != current
        {
            find.highlights.current = current;
            cx.notify();
        }
    }

    /// The find highlights in effect.
    pub fn find_highlights(&self) -> Option<&FindHighlights> {
        self.find.as_ref().map(|f| &f.highlights)
    }

    /// The font code is drawn in: the configured family (or the fallback)
    /// with the [`code_font_features`] of the options.
    pub fn code_font(&self) -> &Font {
        &self.code_font
    }

    pub fn provider(&self) -> &Arc<dyn DiffProvider> {
        &self.provider
    }

    /// The document model: heights, layouts, file states, the anchor.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Applies new options. Nothing on screen moves: layout, wrap and font
    /// changes re-lay files out around the scroll anchor; a theme change
    /// re-highlights; diff or word-diff changes recompute the diffs.
    pub fn set_options(&mut self, opts: ViewportOptions, cx: &mut Context<Self>) {
        self.reveal = None;
        let old = std::mem::replace(&mut self.opts, opts);
        let theme_changed = !Arc::ptr_eq(&old.theme, &self.opts.theme);
        let font_changed = old.code_font != self.opts.code_font
            || old.code_font_size != self.opts.code_font_size
            || old.ligatures != self.opts.ligatures;
        // Pills and labels in the UI font are cached with it.
        let ui_font_changed = old.ui_font != self.opts.ui_font;
        // Large files get no word ranges and no rows, so a new threshold
        // means loading again too.
        let data_changed = old.diff != self.opts.diff
            || old.word_diff != self.opts.word_diff
            || old.large_file_changed_lines != self.opts.large_file_changed_lines;
        if theme_changed || font_changed || ui_font_changed || data_changed {
            self.text_cache.clear();
        }
        if font_changed {
            self.geometry_dirty = true;
        }
        if theme_changed {
            self.pipeline
                .set_theme(self.opts.theme.syntax.clone(), &mut self.doc);
        }
        if data_changed {
            // Files go back to unloaded (heights kept) and reload when near
            // the viewport. Until a file's new data arrives its rows stay in
            // place, blank, and a placeholder shows "Loading…" instead of its
            // stale label (an error, a large-diff count).
            for f in self.pipeline.reload_all(&mut self.doc) {
                self.labels[f as usize] = None;
            }
        }
        if old.diff != self.opts.diff {
            // Line counts depend on the diff options alone.
            self.pipeline.recount(&self.doc, self.opts.diff);
            self.refresh_band_counts();
        }
        if old.syntax != self.opts.syntax {
            self.pipeline.set_syntax(self.opts.syntax);
        }
        cx.notify();
    }

    /// Added and removed lines of file `file_idx`, once known: from its load,
    /// or from the background pass that counts every file after first paint
    /// (design §6.3 "Counts").
    pub fn file_counts(&self, file_idx: u32) -> Option<FileCounts> {
        self.pipeline.counts(file_idx)
    }

    /// Sizes in bytes of file `file_idx`'s old and new blobs (0 for a missing
    /// side), once the background pass has read them.
    pub fn blob_sizes(&self, file_idx: u32) -> Option<(u64, u64)> {
        self.pipeline.blob_sizes(file_idx)
    }

    /// Whether file `file_idx` renders without syntax because a side has more
    /// than 100k lines (design §11.11); the host offers "Highlight anyway".
    pub fn syntax_skipped(&self, file_idx: u32) -> bool {
        self.pipeline.syntax_skipped(file_idx)
    }

    /// "Highlight anyway": highlights file `file_idx` despite its size, with
    /// no line limit and a longer time budget.
    pub fn highlight_anyway(&mut self, file_idx: u32, cx: &mut Context<Self>) {
        self.pipeline.highlight_anyway(file_idx);
        cx.notify();
    }

    /// The background pipeline's counters.
    pub fn pipeline_stats(&self) -> PipelineStats {
        self.pipeline.stats()
    }

    /// Brings `target` to the top of what is visible (clamped to the
    /// document): a file's header to the viewport's top edge; a line (or the
    /// gap hiding it) or a block right below its file's header, which is
    /// pinned there while the file's body scrolls under it. A target in a
    /// file that is not laid out yet lands exactly once it is. A target in a
    /// closed section opens it first, except [`ScrollTarget::Restore`],
    /// which lands on its band. Closes the ⋯ menu.
    pub fn scroll_to(&mut self, target: ScrollTarget, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        let (file_idx, row) = match target {
            ScrollTarget::File(f) => (f, RowKey::Header),
            ScrollTarget::Line {
                file_idx,
                side,
                line,
            }
            | ScrollTarget::Restore {
                file_idx,
                side,
                line,
            } => (file_idx, RowKey::Line { side, line }),
            ScrollTarget::Block(id) => {
                // A block of a file not laid out yet lands once it is.
                let Some(f) = self.blocks.file_of(id) else {
                    return;
                };
                (f, RowKey::Block(id))
            }
        };
        if !matches!(target, ScrollTarget::Restore { .. }) {
            self.open_section_of(file_idx, cx);
        }
        // The viewport's top edge a header's height above a body row, so the
        // pinned header does not cover it. (The first row of a body: the
        // header is in place, at the top edge.)
        let offset_px = match row {
            RowKey::Header => 0.0,
            _ => -self.doc.metrics().header_height,
        };
        self.doc.scroll_to_anchor(ScrollAnchor {
            file_idx,
            row,
            offset_px,
        });
        self.after_scroll(cx);
    }

    /// Restores a scroll position (a line-mapped anchor after a refresh, or
    /// saved view state): `anchor` goes to the viewport's top edge, exactly
    /// once its file is laid out. It never opens a section: an anchor in a
    /// closed one lands on its band. Closes the ⋯ menu.
    pub fn scroll_to_anchor(&mut self, anchor: ScrollAnchor, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        self.doc.scroll_to_anchor(anchor);
        self.after_scroll(cx);
    }

    /// Shows another diff in this view (a live refresh or a new iteration):
    /// `provider`'s files replace the current ones. The options, the code
    /// font, the measured width and layout, the prelude (it is not per file)
    /// and every subscription to the view stay; everything per file starts
    /// over (loads, layouts, collapse, revealed context, flags, blocks, the
    /// cursor, the selection, the ⋯ menu, the sections and the display
    /// order) with the scroll at the top. The host restores
    /// what it keeps ([`DiffViewport::set_collapsed`],
    /// [`DiffViewport::set_expansions`], [`DiffViewport::set_file_flags`],
    /// [`DiffViewport::set_blocks`], [`DiffViewport::scroll_to_anchor`])
    /// before the next frame, so nothing flashes.
    ///
    /// `carry[i] = Some(j)`: new file `i` is old file `j` unchanged (same
    /// paths, modes, blobs and kind). Its loaded rows and tokens are reused,
    /// so it shows at once instead of loading again. Entries that do not
    /// match are ignored. Work still running for the old files is cancelled,
    /// and its results are dropped.
    pub fn set_provider(
        &mut self,
        provider: Arc<dyn DiffProvider>,
        carry: &[Option<u32>],
        cx: &mut Context<Self>,
    ) {
        self.close_menu(cx);
        let files = provider.files();
        let metrics = self.geometry.metrics(
            self.layout,
            self.opts.large_file_changed_lines,
            self.opts.cards,
        );
        let mut doc = Document::new(files.clone(), metrics);
        doc.set_viewport_height(self.doc.viewport_height());
        doc.set_prelude_height(self.doc.prelude_height());
        let mut old_doc = std::mem::replace(&mut self.doc, doc);
        let pipeline = Pipeline::new(
            provider.clone(),
            files.clone(),
            self.opts.theme.syntax.clone(),
        );
        let mut old_pipeline = std::mem::replace(&mut self.pipeline, pipeline);
        old_pipeline.shut_down(&mut old_doc);
        let old_files = std::mem::replace(&mut self.files, files.clone());
        for (f, from) in carry.iter().enumerate() {
            let Some(from) = *from else {
                continue;
            };
            let (Some(new), Some(old)) = (files.get(f), old_files.get(from as usize)) else {
                continue;
            };
            if !same_change(old, new) {
                continue;
            }
            if let FileState::Materialized(file) = old_doc.state(from) {
                let generation = self.doc.begin_loading(f as u32);
                self.doc
                    .set_materialized(f as u32, generation, file.clone());
                self.pipeline.carry(f as u32, &old_pipeline, from);
            }
        }
        self.provider = provider;
        self.special = Specials::default();
        self.labels = files
            .iter()
            .enumerate()
            .map(|(f, c)| self.special.body_label(f as u32, c, None))
            .collect();
        self.layout_keys = vec![None; files.len()];
        self.flags = vec![FileFlags::default(); files.len()];
        self.gaps = Gaps::default();
        self.blocks = Blocks::default();
        self.bands = Vec::new();
        self.cursor = Cursor::default();
        self.selection = None;
        self.drag = None;
        self.pressed = None;
        // Shaped lines are keyed by file index.
        self.text_cache.clear();
        // The next `after_scroll` reports the top file, whatever it is.
        self.top_file = u32::MAX;
        self.after_scroll(cx);
    }

    /// Shows the files in `order` (a permutation of the file indices; anything
    /// else is ignored). Every API stays keyed by `file_idx`. Nothing on
    /// screen moves: the anchor stays put, or at the top.
    pub fn set_order(&mut self, order: Vec<u32>, cx: &mut Context<Self>) {
        self.doc.set_order(order);
        self.after_scroll(cx);
    }

    /// Hides `files` (or shows them again). A hidden file is never laid out,
    /// painted, loaded or walked by the stepwise keys; the background counts
    /// still cover it. An anchor in a file hidden now moves to the top of
    /// where it was, and a cursor, selection or ⋯ menu in one goes.
    pub fn set_hidden(&mut self, files: &[u32], hidden: bool, cx: &mut Context<Self>) {
        self.doc.set_hidden(files, hidden);
        if hidden {
            self.forget_hidden(cx);
        }
        self.after_scroll(cx);
    }

    /// Drops the cursor, the text selection and the ⋯ menu when their file
    /// is hidden: nothing could walk from them or paint them.
    pub(crate) fn forget_hidden(&mut self, cx: &mut Context<Self>) {
        let doc = &self.doc;
        if self.cursor.pos.is_some_and(|p| doc.is_hidden(p.file_idx)) {
            self.cursor.pos = None;
        }
        if self.selection.is_some_and(|s| doc.is_hidden(s.file_idx)) {
            self.selection = None;
        }
        if self.menu_file().is_some_and(|f| self.doc.is_hidden(f)) {
            self.close_menu(cx);
        }
    }

    /// Whether file `file_idx` is hidden ([`DiffViewport::set_hidden`]).
    pub fn is_hidden(&self, file_idx: u32) -> bool {
        self.doc.is_hidden(file_idx)
    }

    /// Every file index in display order ([`DiffViewport::set_order`]).
    pub fn display_order(&self) -> &[u32] {
        self.doc.display_order()
    }

    /// File `file_idx`'s position in display order.
    pub fn display_rank(&self, file_idx: u32) -> u32 {
        self.doc.slot(file_idx)
    }

    /// Scrolls by `dy` pixels (positive = down), clamped to the document.
    /// Closes the ⋯ menu (it would no longer sit under its button).
    pub fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        self.doc.scroll_by(dy);
        self.after_scroll(cx);
    }

    /// Reports a change of the file at the top and repaints. Every change
    /// of the layout or the scroll comes through here, so it settles a
    /// running reveal (ADR-0030 rule 4).
    pub(crate) fn after_scroll(&mut self, cx: &mut Context<Self>) {
        self.reveal = None;
        let top = self.doc.anchor().file_idx;
        if top != self.top_file {
            self.top_file = top;
            cx.emit(ViewportEvent::VisibleFileChanged(top));
        }
        cx.notify();
    }

    pub fn anchor(&self) -> ScrollAnchor {
        *self.doc.anchor()
    }

    /// Split or unified, as painted.
    pub fn effective_layout(&self) -> Layout {
        self.layout
    }

    /// While a panel's width moves (ADR-0030 Panels): resolves what
    /// reshapes at a viewport `target_outer_width` wide now (split or
    /// unified, the wrap width, the gutters and the widths host blocks and
    /// the prelude are measured at) and keeps it until
    /// [`DiffViewport::release_layout`]. Everything else follows the live
    /// width without reshaping: card frames, the split halves (the right one
    /// starts half the live inner width in; each clips its content at its
    /// own edge), right-aligned header controls, the sticky header and row
    /// tints.
    pub fn hold_layout(&mut self, target_outer_width: Pixels, cx: &mut Context<Self>) {
        self.fit_width(target_outer_width.as_f32());
        self.held = true;
        cx.notify();
    }

    /// Ends [`DiffViewport::hold_layout`]: the layout follows the live width
    /// again from the next frame.
    pub fn release_layout(&mut self, cx: &mut Context<Self>) {
        self.held = false;
        cx.notify();
    }

    /// Where file `file_idx`'s code starts on `side`, from its card's inner
    /// left edge (where its rows start: inside the border, or the
    /// viewport's edge in the flat layout), at the current width, layout and
    /// indicators (ADR-0031 C1): the header card's title aligns with it
    /// (C3). A one-sided file has one pane, which either side names. `None`
    /// for no such file.
    pub fn code_x(&self, file_idx: u32, side: Side) -> Option<Pixels> {
        self.files.get(file_idx as usize)?;
        let file = match self.doc.state(file_idx) {
            FileState::Materialized(file) => Some(&**file),
            _ => None,
        };
        let cols = self.columns(file_idx, file);
        Some(px(cols.code_x(cols.code_pane(side))))
    }

    /// File `file_idx`'s card as the last frame painted it: its outer
    /// bounds, borders included, in window coordinates (its quad is cut near
    /// the viewport's edges; these bounds are not). The origin of the
    /// reference tests (ADR-0031). `None` in the flat layout and for a card
    /// the last frame did not paint.
    pub fn card_bounds(&self, file_idx: u32) -> Option<Bounds<Pixels>> {
        let frame = self.frame_pool.as_ref()?;
        let (_, bounds) = frame.card_bounds.iter().find(|(f, _)| *f == file_idx)?;
        Some(*bounds)
    }

    /// Find's per-line match cache as `(live, freed)` (feature
    /// `debug-inspect`): lines whose shaped text is still alive, and lines
    /// the text cache dropped since. `None` while find is off. The cache
    /// holds shaped lines weakly, so it never keeps a dropped line alive.
    #[cfg(feature = "debug-inspect")]
    pub fn debug_find_cache(&self) -> Option<(usize, usize)> {
        self.find.as_ref().map(|f| f.cached_lines())
    }

    /// What the last frame painted (feature `debug-inspect`).
    #[cfg(feature = "debug-inspect")]
    pub fn debug(&self) -> crate::ViewportDebug {
        let (visible_rows, row_bounds, styled_rows) = crate::debug::format_rows(&self.debug_rows);
        crate::ViewportDebug {
            visible_rows,
            anchor: *self.doc.anchor(),
            layout: self.layout,
            shaped_cache_hits: self.text_cache.hits,
            shaped_cache_misses: self.text_cache.misses,
            row_bounds,
            styled_rows,
            painted_text: self
                .debug_text
                .iter()
                .map(|(x, y, t)| (*x, *y, t.text().to_owned()))
                .collect(),
            painted_text_colors: self.debug_text.iter().map(|(_, _, t)| t.color).collect(),
            icons: self
                .frame_pool
                .as_ref()
                .map(|frame| {
                    let o = frame.origin;
                    frame
                        .icons
                        .iter()
                        .map(|(b, path, _, _)| crate::debug::IconDebug {
                            path: path.to_string(),
                            bounds: (
                                (b.origin.x - o.x).as_f32(),
                                (b.origin.y - o.y).as_f32(),
                                b.size.width.as_f32(),
                                b.size.height.as_f32(),
                            ),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            headers: self.debug_headers.clone(),
            bands: self.debug_bands.clone(),
            cards: self
                .frame_pool
                .as_ref()
                .map(|frame| {
                    let o = frame.origin;
                    frame
                        .card_bounds
                        .iter()
                        .map(|(f, b)| crate::debug::CardDebug {
                            file_idx: *f,
                            bounds: (
                                (b.origin.x - o.x).as_f32(),
                                (b.origin.y - o.y).as_f32(),
                                b.size.width.as_f32(),
                                b.size.height.as_f32(),
                            ),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            controls: self
                .frame_pool
                .as_ref()
                .map(|frame| {
                    let o = frame.origin;
                    frame
                        .controls
                        .iter()
                        .map(|c| crate::debug::ControlDebug {
                            action: c.action,
                            bounds: (
                                (c.bounds.origin.x - o.x).as_f32(),
                                (c.bounds.origin.y - o.y).as_f32(),
                                c.bounds.size.width.as_f32(),
                                c.bounds.size.height.as_f32(),
                            ),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            plus_button: self.frame_pool.as_ref().and_then(|f| f.plus).map(|p| {
                crate::debug::PlusDebug {
                    file_idx: p.file_idx,
                    side: p.side,
                    line: p.line,
                    bounds: p.bounds,
                }
            }),
            menu: self.menu.as_ref().map(|m| crate::debug::MenuDebug {
                file_idx: m.file_idx,
                items: m.items.iter().map(|(l, e)| ((*l).to_owned(), *e)).collect(),
            }),
            prelude: self.frame_pool.as_ref().and_then(|frame| {
                let p = frame.prelude.as_ref()?;
                let o = frame.origin;
                Some((
                    (p.origin.x - o.x).as_f32(),
                    (p.origin.y - o.y).as_f32(),
                    p.width,
                    p.height,
                ))
            }),
            slots: self
                .frame_pool
                .as_ref()
                .map(|f| f.slots.clone())
                .unwrap_or_default(),
            reveal: self.frame_pool.as_ref().and_then(|f| f.reveal),
            chevrons: self
                .frame_pool
                .as_ref()
                .map(|f| f.chevrons.clone())
                .unwrap_or_default(),
            relayouts: self.relayouts,
        }
    }

    /// Prepaint: fits the document to `bounds` (unless its layout is held),
    /// lays out and loads files near the viewport, and builds the frame's
    /// display list. A reveal's commit frame first builds the settled frame
    /// (discarded), so whatever the motion uncovers is laid out and shaped
    /// now and its later frames shape nothing.
    pub(crate) fn prepare_frame(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        if self.geometry_dirty {
            (self.code_font, self.geometry) = resolve_font(&self.opts, window);
            self.geometry_dirty = false;
        }
        let (width, height) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
        if !self.held {
            self.fit_width(width);
        }
        if self.doc.viewport_height() != height {
            self.doc.set_viewport_height(height);
        }
        self.text_cache.begin_frame();
        self.prepare_window(height, cx);
        // A cursor move into a file that was not laid out lands now; if it
        // scrolled, the files around the new position are laid out too.
        if self.resolve_pending_cursor(cx) {
            self.prepare_window(height, cx);
        }
        let pointer = self.pointer_inside.then(|| {
            let p = window.mouse_position();
            (
                (p.x - bounds.origin.x).as_f32(),
                (p.y - bounds.origin.y).as_f32(),
            )
        });
        let marks = Marks {
            cursor: self.cursor.pos,
            selection: self.selection,
            pointer,
            text_drag: self.drag == Some(Drag::Text),
            old_side_comments: self.old_side_comments,
        };

        let scale = window.scale_factor().max(1.0);
        let mut frame = self.frame_pool.take().unwrap_or_default();
        let misses = self.text_cache.misses;
        let reveal = self.reveal_geometry(scale, self.snapped_scroll(scale), cx);
        if reveal.is_some() && self.take_reveal_commit() {
            let mut settled = Frame::default();
            self.build_frame(&mut settled, bounds, marks, scale, None, window, cx);
        }
        self.build_frame(&mut frame, bounds, marks, scale, reveal, window, cx);
        // Every pass's cache misses: lines shaped by a pass that was then
        // rebuilt were still shaped this frame.
        frame.shaped = (self.text_cache.misses - misses) as u32;
        self.follow_menu_button(&frame, window, cx);
        frame
    }

    /// Fills `frame` with the rows intersecting the viewport (and `reveal`'s
    /// motion), laying it out again while wrapped rows measure other heights
    /// than estimated.
    #[allow(clippy::too_many_arguments)]
    fn build_frame(
        &mut self,
        frame: &mut Frame,
        bounds: Bounds<Pixels>,
        marks: Marks,
        scale: f32,
        reveal: Option<RevealGeom>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prelude_width = self.prelude_width();
        let scroll_top = self.snapped_scroll(scale);
        for pass in 0..WRAP_PASSES {
            frame.clear();
            #[cfg(feature = "debug-inspect")]
            {
                self.debug_rows.clear();
                self.debug_headers.clear();
                self.debug_bands.clear();
                self.debug_text.clear();
            }
            let mut painter = Painter {
                doc: &self.doc,
                files: &self.files,
                file_labels: &self.labels,
                flags: &self.flags,
                gaps: &self.gaps,
                special: &self.special,
                theme: &self.opts.theme,
                style: self.opts.style,
                syntax: self.opts.syntax,
                word_diff: self.opts.word_diff.is_some(),
                geometry: self.geometry,
                font: &self.code_font,
                ui_font: &self.opts.ui_font,
                layout: self.layout,
                bounds,
                inner: inner_bounds(bounds, self.opts.cards),
                layout_width: self.width,
                prelude_width,
                cards: self.opts.cards,
                prelude: self.prelude.as_ref().map(|p| &p.render),
                scroll_top,
                reveal,
                in_reveal: false,
                rows_clip: None,
                cache: &mut self.text_cache,
                pipeline: &self.pipeline,
                blocks: &self.blocks,
                bands: &self.bands,
                find: self.find.as_mut(),
                text_system: window.text_system().clone(),
                marks,
                frame: &mut *frame,
                corrections: Vec::new(),
                #[cfg(feature = "debug-inspect")]
                debug: &mut self.debug_rows,
                #[cfg(feature = "debug-inspect")]
                debug_headers: &mut self.debug_headers,
                #[cfg(feature = "debug-inspect")]
                debug_bands: &mut self.debug_bands,
                #[cfg(feature = "debug-inspect")]
                debug_text: &mut self.debug_text,
            };
            painter.paint_visible();
            let corrections = painter.corrections;
            if corrections.is_empty() {
                break;
            }
            // Wrapped rows measured taller or shorter than estimated: fix the
            // heights (the anchor keeps the content still) and lay out again.
            for (f, row, h) in corrections {
                self.doc.set_row_height(f, row, h);
            }
            if pass + 1 == WRAP_PASSES {
                // Out of passes: this frame shows the old heights, the next
                // one the fixed ones (and measures whatever they reveal).
                let view = cx.entity_id();
                window.on_next_frame(move |_, cx| cx.notify(view));
            }
        }
    }

    /// After paint: keeps the frame's buffers for the next one and reports
    /// its timing.
    pub(crate) fn finish_frame(&mut self, frame: Frame, stats: FrameStats, cx: &mut Context<Self>) {
        self.frame_pool = Some(frame);
        // First paint: every visible row is on screen. Only now do the blob
        // sizes and line counts of every other file start (design §6.3).
        if stats.loading_rows == 0 && !self.pipeline.background_started() {
            self.pipeline.start_background(self.opts.diff, cx);
        }
        cx.emit(ViewportEvent::FrameStats(stats));
    }

    /// Re-decides the layout for a viewport `outer_width` px wide, by the
    /// rows' width (a card's inner width).
    fn fit_width(&mut self, outer_width: f32) {
        let (_, width) = inner_span(outer_width, self.opts.cards);
        let columns = width / self.geometry.advance;
        let previous = self.measured.then_some(self.layout);
        self.layout = resolve_layout(
            self.opts.layout,
            columns,
            self.opts.split_min_columns,
            previous,
        );
        self.measured = true;
        self.width = width;
        self.outer_width = outer_width;
        let metrics = self.geometry.metrics(
            self.layout,
            self.opts.large_file_changed_lines,
            self.opts.cards,
        );
        if *self.doc.metrics() != metrics {
            self.doc.set_metrics(metrics);
            #[cfg(feature = "debug-inspect")]
            {
                self.relayouts += 1;
            }
        }
    }

    /// Schedules background work for the materialization window and lays
    /// out (synchronously) the files in it whose data is there.
    fn prepare_window(&mut self, height: f32, cx: &mut Context<Self>) {
        self.pipeline
            .schedule(&mut self.doc, &self.files, &self.opts, self.layout, cx);
        let range = self.doc.materialize_range(height, self.opts.window_screens);
        // Laying a file out changes heights: collect the files first.
        let files: Vec<u32> = self.doc.shown_files(range).collect();
        for f in files {
            if !self.doc.is_collapsed(f) {
                self.ensure_layout(f);
            }
        }
    }

    /// Lays file `f` out when it has no layout or a stale one and its data
    /// is there; whether it is laid out for the current options now.
    pub(crate) fn ensure_layout(&mut self, f: u32) -> bool {
        self.apply_deferred_reveals(f);
        let key = self.layout_key(f);
        let stale = self.doc.file_layout(f).is_none() || self.layout_keys[f as usize] != Some(key);
        if stale && let Some(layout) = self.build_layout(f, key) {
            self.doc.set_file_layout(f, layout);
            self.layout_keys[f as usize] = Some(key);
        }
        self.doc.file_layout(f).is_some() && self.layout_keys[f as usize] == Some(key)
    }

    /// The columns of file `f`'s rows ([`Columns::for_file`]).
    fn columns(&self, f: u32, file: Option<&MaterializedFile>) -> Columns {
        let lines = file.map_or(0, |m| m.diff.old.len().max(m.diff.new.len()));
        Columns::new(
            self.layout,
            0.0,
            self.width,
            self.geometry.advance,
            digits(lines),
            self.opts.style.indicators,
        )
        .for_file(&self.files[f as usize])
    }

    /// The layout file `f` is drawn in: unified for a one-sided file in
    /// either layout ([`layout_for`]).
    pub(crate) fn file_layout_mode(&self, f: u32) -> Layout {
        layout_for(&self.files[f as usize], self.layout)
    }

    fn layout_key(&self, f: u32) -> LayoutKey {
        let wrap = match (self.opts.style.wrap, self.doc.state(f)) {
            (true, FileState::Materialized(file)) => {
                let cols = self.columns(f, Some(file));
                let pane = match cols.layout {
                    Layout::Split => Pane::Half(0),
                    Layout::Unified => Pane::Full,
                };
                cols.code_width(pane).floor() as u32
            }
            _ => 0,
        };
        let load_requested = self.special.load_requested(f);
        LayoutKey {
            layout: self.file_layout_mode(f),
            wrap,
            large_file_changed_lines: if load_requested {
                u32::MAX
            } else {
                self.opts.large_file_changed_lines
            },
            expansions: self.gaps.version(f),
            load_requested,
        }
    }

    /// Lays file `f` out again now (revealed context, "Load diff"), or on the
    /// next frame once its data is there.
    pub(crate) fn relayout(&mut self, f: u32) {
        self.layout_keys[f as usize] = None;
        self.apply_deferred_reveals(f);
        let key = self.layout_key(f);
        if let Some(layout) = self.build_layout(f, key) {
            self.doc.set_file_layout(f, layout);
            self.layout_keys[f as usize] = Some(key);
        }
    }

    /// File `f`'s body rows for `key`, or `None` while its data is not there.
    fn build_layout(&mut self, f: u32, key: LayoutKey) -> Option<FileLayout> {
        let metrics = self.doc.metrics().clone();
        let change = &self.files[f as usize];
        if let Some(label) = self
            .special
            .body_label(f, change, self.pipeline.blob_sizes(f))
        {
            let h = if change.kind == FileKind::Submodule {
                metrics.row_height
            } else {
                metrics.placeholder_height
            };
            self.labels[f as usize] = Some(label);
            return Some(FileLayout::placeholder(h));
        }
        if !needs_blobs(change, key.load_requested) {
            return Some(FileLayout::new(Vec::new(), &[]));
        }
        let file = match self.doc.state(f) {
            FileState::Materialized(file) => file.clone(),
            // The error replaces whatever the file showed before (rows kept
            // while it reloaded, or an estimate).
            FileState::Failed(msg) => {
                self.labels[f as usize] = Some(BodyLabel::plain(failed_label(msg)));
                return Some(FileLayout::placeholder(metrics.placeholder_height));
            }
            _ => return None,
        };
        self.special.note_lfs(f, &file);
        let changed = file.diff.additions + file.diff.deletions;
        if changed > key.large_file_changed_lines {
            self.labels[f as usize] = Some(large_label(changed));
            return Some(FileLayout::placeholder(metrics.placeholder_height));
        }
        self.labels[f as usize] = None;
        let rows = self.gaps.rows(f, &file, key.layout);
        // Split by the layout mode rather than the rows' contents (a split
        // file of gaps alone holds no split rows): `with_blocks` pairs old-
        // and new-side blocks by it.
        let layout = FileLayout::from_rows(rows, &metrics).with_split(key.layout == Layout::Split);
        if key.wrap == 0 {
            return Some(layout);
        }
        let heights = wrapped_heights(&layout, rows, &file, self.geometry.advance, key.wrap as f32);
        Some(layout.with_heights(&heights))
    }

    /// A worker finished a job (`None`: it found nothing to do). Takes the
    /// result in, queues what follows from it, and says whether the worker
    /// keeps going.
    pub(crate) fn pipeline_done(&mut self, done: Option<Done>, cx: &mut Context<Self>) -> bool {
        let had_job = done.is_some();
        if let Some(done) = done {
            let h = self.doc.viewport_height();
            let window = self.doc.materialize_range(h, self.opts.window_screens);
            let applied = self.pipeline.apply(done, &mut self.doc, window);
            let repaint = applied.repaint;
            self.take_applied(applied, cx);
            // Follow-up work (a loaded file's highlights, files that entered
            // the window as estimates shrank) is queued at once rather than on
            // the next frame.
            self.pipeline
                .schedule(&mut self.doc, &self.files, &self.opts, self.layout, cx);
            if repaint {
                cx.notify();
            }
        }
        self.pipeline.worker_continues(had_job)
    }

    fn take_applied(&mut self, applied: Applied, cx: &mut Context<Self>) {
        if applied.counts {
            if self.refresh_band_counts() {
                cx.notify();
            }
            cx.emit(ViewportEvent::CountsUpdated);
        }
        for f in applied.relayout {
            // New rows (or an error): lay the file out on the next frame.
            self.layout_keys[f as usize] = None;
        }
        for f in applied.binary {
            self.mark_binary(f, cx);
        }
        if applied.grew {
            let h = self.doc.viewport_height();
            let keep = self.doc.materialize_range(h, self.opts.window_screens);
            let budget = self.opts.eviction_budget_bytes;
            for f in self.doc.evict_over_budget_keeping(budget, keep) {
                self.layout_keys[f as usize] = None;
                self.gaps.forget_rows(f);
                self.pipeline.forget(f);
            }
        }
    }

    /// File `f` was listed as text but has binary content: it shows the
    /// binary placeholder from now on, and the host hears about it.
    fn mark_binary(&mut self, f: u32, cx: &mut Context<Self>) {
        if self.files[f as usize].kind == FileKind::Binary {
            return;
        }
        // Let go of the shared list first, so the document changes its copy
        // in place (it copies the list only the first time).
        self.files = Arc::default();
        self.doc.set_kind(f, FileKind::Binary);
        self.files = self.doc.files().clone();
        self.layout_keys[f as usize] = None;
        cx.emit(ViewportEvent::BinaryDetected(f));
    }
}

impl Render for DiffViewport {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(DiffElement::new(cx.entity()))
            .children(self.menu_element())
    }
}

/// Whether two file changes show the same thing (everything but their
/// index), so one's loaded data serves the other.
fn same_change(a: &FileChange, b: &FileChange) -> bool {
    a.status == b.status
        && a.old_path == b.old_path
        && a.new_path == b.new_path
        && a.old_mode == b.old_mode
        && a.new_mode == b.new_mode
        && a.old_blob == b.old_blob
        && a.new_blob == b.new_blob
        && a.kind == b.kind
        && a.generated == b.generated
}

/// The OpenType features the code font is drawn with: none with `ligatures`
/// (the font's defaults), else contextual alternates (`calt`, where fonts
/// like Lilex keep their programming ligatures) and standard ligatures
/// (`liga`) off, so a diff shows `->`, `!=` and `>=` as typed.
pub fn code_font_features(ligatures: bool) -> FontFeatures {
    if ligatures {
        FontFeatures::default()
    } else {
        FontFeatures(Arc::new(vec![("calt".into(), 0), ("liga".into(), 0)]))
    }
}

/// Code font `family` with the [`code_font_features`] for `ligatures`.
pub fn code_font(family: impl Into<SharedString>, ligatures: bool) -> Font {
    Font {
        features: code_font_features(ligatures),
        ..font(family)
    }
}

/// The code font (the configured family, or the system monospaced font for
/// one of its aliases or when the family is missing) and its geometry.
///
/// Resolving a font falls back to GPUI's default stack when the family is not
/// installed; the family the result belongs to tells which happened. This is
/// on the first-paint path, so it resolves one font instead of listing every
/// installed family.
fn resolve_font(opts: &ViewportOptions, window: &Window) -> (Font, Geometry) {
    let text_system = window.text_system();
    let family = crate::kit::code_family(&opts.code_font);
    let family = if crate::kit::is_installed(text_system, family) {
        family
    } else {
        crate::kit::SYSTEM_MONO_FONT
    };
    let code = code_font(SharedString::from(family.to_owned()), opts.ligatures);
    let font_id = text_system.resolve_font(&code);
    let size = opts.code_font_size.max(1.0);
    // Without an advance, `Geometry::new` falls back to a typical one.
    let advance = text_system
        .ch_advance(font_id, px(size))
        .map_or(f32::NAN, |a| a.as_f32());
    (code, Geometry::new(size, advance))
}
