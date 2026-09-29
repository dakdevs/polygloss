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
    Bounds, Context, EventEmitter, Font, IntoElement, Pixels, Render, SharedString, Window, font,
    px,
};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::Layout;
use polygloss_diff::word::Granularity;
use polygloss_diff::{FileChange, FileKind, Side};

use crate::document::{
    BlockId, DEFAULT_EVICTION_BUDGET_BYTES, DEFAULT_WINDOW_SCREENS, Document, FileLayout,
    FileState, RowKey, ScrollAnchor,
};
use crate::element::DiffElement;
use crate::layout::{Columns, Geometry, LayoutMode, Pane, digits, resolve_layout, wrapped_heights};
use crate::materialize::MaterializedFile;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::DebugRow;
use crate::paint_rows::{Frame, Painter, failed_label, special_label};
use crate::pipeline::{Applied, Done, FileCounts, Pipeline, PipelineStats, needs_blobs};
use crate::provider::DiffProvider;
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

/// Where [`DiffViewport::scroll_to`] puts the viewport's top edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    /// A file's header.
    File(u32),
    /// The row showing `line` (0-based) of `side`, or the gap hiding it.
    Line {
        file_idx: u32,
        side: Side,
        line: u32,
    },
    /// A host block (T2.7).
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
    large_file_changed_lines: u32,
}

/// The GPUI view over one diff.
pub struct DiffViewport {
    provider: Arc<dyn DiffProvider>,
    opts: ViewportOptions,
    files: Arc<Vec<FileChange>>,
    doc: Document,
    geometry: Geometry,
    code_font: Font,
    geometry_dirty: bool,
    /// The effective layout; `measured` once a frame has seen the width.
    layout: Layout,
    measured: bool,
    width: f32,
    layout_keys: Vec<Option<LayoutKey>>,
    /// What a file's body shows when it has no code rows (special files,
    /// large diffs).
    labels: Vec<Option<SharedString>>,
    text_cache: TextCache,
    pipeline: Pipeline,
    frame_pool: Option<Frame>,
    top_file: u32,
    #[cfg(feature = "debug-inspect")]
    debug_rows: Vec<DebugRow>,
    #[cfg(feature = "debug-inspect")]
    debug_text: Vec<(f32, f32, std::rc::Rc<crate::text_cache::ShapedText>)>,
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
        let labels = files
            .iter()
            .map(|c| special_label(c).map(SharedString::from))
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
            #[cfg(feature = "debug-inspect")]
            debug_rows: Vec::new(),
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
            for f in self.pipeline.reload_all(&mut self.doc, self.opts.diff) {
                self.labels[f as usize] = None;
            }
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

    /// Puts `target` at the top of the viewport (clamped to the document).
    /// A target in a file that is not laid out yet lands exactly once it is.
    pub fn scroll_to(&mut self, target: ScrollTarget, cx: &mut Context<Self>) {
        match target {
            ScrollTarget::File(f) => self.doc.scroll_to(f, RowKey::Header),
            ScrollTarget::Line {
                file_idx,
                side,
                line,
            } => self.doc.scroll_to(file_idx, RowKey::Line { side, line }),
            ScrollTarget::Block(id) => {
                let Some(f) = (0..self.doc.len()).find(|&f| {
                    self.doc
                        .file_layout(f)
                        .is_some_and(|l| l.find(RowKey::Block(id)).is_some())
                }) else {
                    return;
                };
                self.doc.scroll_to(f, RowKey::Block(id));
            }
        }
        self.after_scroll(cx);
    }

    /// Scrolls by `dy` pixels (positive = down), clamped to the document.
    pub fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        self.doc.scroll_by(dy);
        self.after_scroll(cx);
    }

    fn after_scroll(&mut self, cx: &mut Context<Self>) {
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

        let scale = f64::from(window.scale_factor().max(1.0));
        let mut frame = self.frame_pool.take().unwrap_or_default();
        let misses = self.text_cache.misses;
        for pass in 0..WRAP_PASSES {
            frame.clear();
            #[cfg(feature = "debug-inspect")]
            {
                self.debug_rows.clear();
                self.debug_text.clear();
            }
            let mut painter = Painter {
                doc: &self.doc,
                files: &self.files,
                file_labels: &self.labels,
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
                text_system: window.text_system().clone(),
                frame: &mut frame,
                corrections: Vec::new(),
                #[cfg(feature = "debug-inspect")]
                debug: &mut self.debug_rows,
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
        LayoutKey {
            layout: self.layout,
            wrap,
            large_file_changed_lines: self.opts.large_file_changed_lines,
        }
    }

    /// File `f`'s body rows for `key`, or `None` while its data is not there.
    fn build_layout(&mut self, f: u32, key: LayoutKey) -> Option<FileLayout> {
        let metrics = self.doc.metrics().clone();
        let change = &self.files[f as usize];
        if let Some(label) = special_label(change) {
            let h = if change.kind == FileKind::Submodule {
                metrics.row_height
            } else {
                metrics.placeholder_height
            };
            self.labels[f as usize] = Some(label.into());
            return Some(FileLayout::placeholder(h));
        }
        if !needs_blobs(change) {
            return Some(FileLayout::new(Vec::new(), &[]));
        }
        let file = match self.doc.state(f) {
            FileState::Materialized(file) => file,
            // The error replaces whatever the file showed before (rows kept
            // while it reloaded, or an estimate).
            FileState::Failed(msg) => {
                self.labels[f as usize] = Some(failed_label(msg).into());
                return Some(FileLayout::placeholder(metrics.placeholder_height));
            }
            _ => return None,
        };
        let changed = file.diff.additions + file.diff.deletions;
        if changed > key.large_file_changed_lines {
            self.labels[f as usize] = Some(format!("Large diff · {changed} changed lines").into());
            return Some(FileLayout::placeholder(metrics.placeholder_height));
        }
        self.labels[f as usize] = None;
        let rows = file.rows(key.layout);
        let layout = FileLayout::from_rows(rows, &metrics);
        if key.wrap == 0 {
            return Some(layout);
        }
        let heights = wrapped_heights(&layout, rows, file, self.geometry.advance, key.wrap as f32);
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
        DiffElement::new(cx.entity())
    }
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
    let wanted = font(opts.code_font.clone());
    let wanted_id = text_system.resolve_font(&wanted);
    let installed = text_system
        .get_font_for_id(wanted_id)
        .is_some_and(|resolved| resolved.family == wanted.family);
    let code = if installed {
        wanted
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
