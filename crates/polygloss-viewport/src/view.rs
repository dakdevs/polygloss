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
    Bounds, Context, EventEmitter, Font, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, Window, div, font, px,
};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::Layout;
use polygloss_diff::word::Granularity;
use polygloss_diff::{FileChange, FileKind, Side};

use crate::blocks::Blocks;
use crate::controls::Pressed;
use crate::cursor::Cursor;
use crate::document::{
    BlockId, DEFAULT_EVICTION_BUDGET_BYTES, DEFAULT_WINDOW_SCREENS, Document, FileLayout,
    FileState, RowKey, ScrollAnchor,
};
use crate::element::DiffElement;
use crate::file_flags::FileFlags;
use crate::gap::Gaps;
use crate::header::HeaderMenu;
use crate::layout::{Columns, Geometry, LayoutMode, Pane, digits, resolve_layout, wrapped_heights};
use crate::materialize::MaterializedFile;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::DebugRow;
use crate::paint_rows::{Frame, Marks, Painter, failed_label};
use crate::pipeline::{Applied, Done, FileCounts, Pipeline, PipelineStats};
use crate::provider::DiffProvider;
use crate::selection::{Drag, TextSelection};
use crate::special::{BodyLabel, Specials, large_label, needs_blobs};
use crate::style::{DiffStyle, ViewportTheme};
use crate::text_cache::{TEXT_CACHE_CAPACITY, TextCache};

/// Font used when the configured code font is not installed.
const FALLBACK_CODE_FONT: &str = "Menlo";

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
    /// Code font family (design §18: Lilex; Menlo when it is not installed).
    pub code_font: SharedString,
    pub code_font_size: f32,
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
            code_font_size: 13.0,
            theme: Arc::new(ViewportTheme::default()),
            large_file_changed_lines: 20_000,
            syntax: true,
            window_screens: DEFAULT_WINDOW_SCREENS,
            eviction_budget_bytes: DEFAULT_EVICTION_BUDGET_BYTES,
        }
    }
}

/// What [`DiffViewport::scroll_to`] brings to the top of what is visible.
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
    pub(crate) width: f32,
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
    /// The control a mouse button went down on (a click needs the release
    /// there too).
    pub(crate) pressed: Option<Pressed>,
    /// Host blocks (threads, composers, notes; see [`crate::blocks`]).
    pub(crate) blocks: Blocks,
    /// The line cursor and range ([`crate::cursor`]).
    pub(crate) cursor: Cursor,
    /// Selected text ([`crate::selection`]).
    pub(crate) selection: Option<TextSelection>,
    /// A mouse drag across line numbers or code.
    pub(crate) drag: Option<Drag>,
    /// The pointer is over the viewport (not a header or a popup over it).
    pub(crate) pointer_inside: bool,
    #[cfg(feature = "debug-inspect")]
    pub(crate) debug_rows: Vec<DebugRow>,
    #[cfg(feature = "debug-inspect")]
    pub(crate) debug_headers: Vec<crate::debug::HeaderDebug>,
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
        let width = window.viewport_size().width.as_f32();
        let layout = resolve_layout(
            opts.layout,
            width / geometry.advance,
            opts.split_min_columns,
            None,
        );
        let doc = Document::new(
            files.clone(),
            geometry.metrics(layout, opts.large_file_changed_lines),
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
            labels,
            text_cache: TextCache::new(TEXT_CACHE_CAPACITY),
            frame_pool: None,
            top_file: 0,
            flags: vec![FileFlags::default(); file_count],
            gaps: Gaps::default(),
            special,
            menu: None,
            pressed: None,
            blocks: Blocks::default(),
            cursor: Cursor::default(),
            selection: None,
            drag: None,
            pointer_inside: false,
            #[cfg(feature = "debug-inspect")]
            debug_rows: Vec::new(),
            #[cfg(feature = "debug-inspect")]
            debug_headers: Vec::new(),
            #[cfg(feature = "debug-inspect")]
            debug_text: Vec::new(),
        }
    }

    pub fn options(&self) -> &ViewportOptions {
        &self.opts
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
        let old = std::mem::replace(&mut self.opts, opts);
        let theme_changed = !Arc::ptr_eq(&old.theme, &self.opts.theme);
        let font_changed =
            old.code_font != self.opts.code_font || old.code_font_size != self.opts.code_font_size;
        // Large files get no word ranges and no rows, so a new threshold
        // means loading again too.
        let data_changed = old.diff != self.opts.diff
            || old.word_diff != self.opts.word_diff
            || old.large_file_changed_lines != self.opts.large_file_changed_lines;
        if theme_changed || font_changed || data_changed {
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
    /// file that is not laid out yet lands exactly once it is. Closes the ⋯
    /// menu.
    pub fn scroll_to(&mut self, target: ScrollTarget, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        let (file_idx, row) = match target {
            ScrollTarget::File(f) => (f, RowKey::Header),
            ScrollTarget::Line {
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
    /// once its file is laid out. Closes the ⋯ menu.
    pub fn scroll_to_anchor(&mut self, anchor: ScrollAnchor, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        self.doc.scroll_to_anchor(anchor);
        self.after_scroll(cx);
    }

    /// Shows another diff in this view (a live refresh or a new iteration):
    /// `provider`'s files replace the current ones. The options, the code
    /// font, the measured width and layout stay, and so does every
    /// subscription to the view; everything per file starts over (loads,
    /// layouts, collapse, revealed context, flags, blocks, the cursor, the
    /// selection, the ⋯ menu) with the scroll at the top. The host restores
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
        let metrics = self
            .geometry
            .metrics(self.layout, self.opts.large_file_changed_lines);
        let mut doc = Document::new(files.clone(), metrics);
        doc.set_viewport_height(self.doc.viewport_height());
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

    /// Scrolls by `dy` pixels (positive = down), clamped to the document.
    /// Closes the ⋯ menu (it would no longer sit under its button).
    pub fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        self.doc.scroll_by(dy);
        self.after_scroll(cx);
    }

    /// Reports a change of the file at the top and repaints.
    pub(crate) fn after_scroll(&mut self, cx: &mut Context<Self>) {
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
            headers: self.debug_headers.clone(),
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
        }
    }

    /// Prepaint: fits the document to `bounds`, lays out and loads files near
    /// the viewport, and builds the frame's display list.
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
        self.fit_width(width);
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
        };

        let scale = f64::from(window.scale_factor().max(1.0));
        let mut frame = self.frame_pool.take().unwrap_or_default();
        let misses = self.text_cache.misses;
        for pass in 0..WRAP_PASSES {
            frame.clear();
            #[cfg(feature = "debug-inspect")]
            {
                self.debug_rows.clear();
                self.debug_headers.clear();
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
                layout: self.layout,
                bounds,
                scroll_top: (self.doc.scroll_top() * scale).round() / scale,
                cache: &mut self.text_cache,
                pipeline: &self.pipeline,
                blocks: &self.blocks,
                text_system: window.text_system().clone(),
                marks,
                frame: &mut frame,
                corrections: Vec::new(),
                #[cfg(feature = "debug-inspect")]
                debug: &mut self.debug_rows,
                #[cfg(feature = "debug-inspect")]
                debug_headers: &mut self.debug_headers,
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
        // Every pass's cache misses: lines shaped by a pass that was then
        // rebuilt were still shaped this frame.
        frame.shaped = (self.text_cache.misses - misses) as u32;
        self.follow_menu_button(&frame, window, cx);
        frame
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

    /// Re-decides the layout for a viewport `width` px wide.
    fn fit_width(&mut self, width: f32) {
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
        let metrics = self
            .geometry
            .metrics(self.layout, self.opts.large_file_changed_lines);
        if *self.doc.metrics() != metrics {
            self.doc.set_metrics(metrics);
        }
    }

    /// Schedules background work for the materialization window and lays
    /// out (synchronously) the files in it whose data is there.
    fn prepare_window(&mut self, height: f32, cx: &mut Context<Self>) {
        self.pipeline
            .schedule(&mut self.doc, &self.files, &self.opts, self.layout, cx);
        let range = self.doc.materialize_range(height, self.opts.window_screens);
        for f in range {
            if self.doc.is_collapsed(f) {
                continue;
            }
            let key = self.layout_key(f);
            let stale =
                self.doc.file_layout(f).is_none() || self.layout_keys[f as usize] != Some(key);
            if stale && let Some(layout) = self.build_layout(f, key) {
                self.doc.set_file_layout(f, layout);
                self.layout_keys[f as usize] = Some(key);
            }
        }
    }

    fn columns(&self, file: Option<&MaterializedFile>) -> Columns {
        let lines = file.map_or(0, |m| m.diff.old.len().max(m.diff.new.len()));
        Columns::new(
            self.layout,
            self.width,
            self.geometry.advance,
            digits(lines),
            self.opts.style.indicators,
        )
    }

    fn layout_key(&self, f: u32) -> LayoutKey {
        let wrap = match (self.opts.style.wrap, self.doc.state(f)) {
            (true, FileState::Materialized(file)) => {
                let cols = self.columns(Some(file));
                let pane = match self.layout {
                    Layout::Split => Pane::Half(0),
                    Layout::Unified => Pane::Full,
                };
                cols.code_width(pane).floor() as u32
            }
            _ => 0,
        };
        let load_requested = self.special.load_requested(f);
        LayoutKey {
            layout: self.layout,
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
        let layout = FileLayout::from_rows(rows, &metrics);
        if key.wrap == 0 {
            return Some(layout);
        }
        let heights = wrapped_heights(&layout, rows, &file, self.geometry.advance, key.wrap as f32);
        Some(FileLayout::new(layout.rows().to_vec(), &heights))
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

/// The code font (the configured family, or Menlo when it is missing) and its
/// geometry.
///
/// Resolving a font falls back to GPUI's default stack when the family is not
/// installed; the family the result belongs to tells which happened. This is
/// on the first-paint path, so it resolves one font instead of listing every
/// installed family.
fn resolve_font(opts: &ViewportOptions, window: &Window) -> (Font, Geometry) {
    let text_system = window.text_system();
    let code = if crate::kit::is_installed(text_system, &opts.code_font) {
        font(opts.code_font.clone())
    } else {
        font(FALLBACK_CODE_FONT)
    };
    let font_id = text_system.resolve_font(&code);
    let size = opts.code_font_size.max(1.0);
    let advance = text_system
        .ch_advance(font_id, px(size))
        .map(|a| a.as_f32())
        .unwrap_or(0.6 * size);
    (code, Geometry::new(size, advance))
}
