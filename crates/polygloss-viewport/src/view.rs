//! `DiffViewport`: the GPUI view over one diff (design §11.6, §12.4).
//!
//! It owns the [`Document`] (every file's height and the logical scroll
//! anchor), the shaped-line cache and the background work that turns files
//! near the viewport into [`MaterializedFile`]s: blobs → diff → word ranges →
//! rows (swapped in, laid out) → syntax tokens (swapped in without moving
//! anything). T2.6 turns that loading into the prioritized, cancellable
//! pipeline; the view only ever sees results through the document's
//! generation-checked states.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Bounds, Context, EventEmitter, Font, IntoElement, Pixels, Render,
    SharedString, Task, Window, font, px,
};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::Layout;
use polygloss_diff::word::Granularity;
use polygloss_diff::{FileChange, FileKind, Side};
use polygloss_highlight::{Budget, Highlighter, Tokens, guess_language};

use crate::document::{
    BlockId, DEFAULT_EVICTION_BUDGET_BYTES, DEFAULT_WINDOW_SCREENS, Document, FileLayout,
    FileState, RowKey, ScrollAnchor, SizeHint,
};
use crate::element::DiffElement;
use crate::layout::{Columns, Geometry, LayoutMode, Pane, digits, resolve_layout, wrapped_heights};
use crate::materialize::MaterializedFile;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::DebugRow;
use crate::paint_rows::{Frame, Painter, failed_label, special_label};
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
    highlighter: Arc<Highlighter>,
    loads: HashMap<u32, Task<()>>,
    highlights: HashMap<u32, Task<()>>,
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
            highlighter: Arc::new(Highlighter::new(opts.theme.syntax.clone())),
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
            loads: HashMap::new(),
            highlights: HashMap::new(),
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
        let data_changed = old.diff != self.opts.diff || old.word_diff != self.opts.word_diff;
        if theme_changed || font_changed || data_changed {
            self.text_cache.clear();
        }
        if font_changed {
            self.geometry_dirty = true;
        }
        if theme_changed {
            self.highlighter = Arc::new(Highlighter::new(self.opts.theme.syntax.clone()));
            self.highlights.clear();
            if old.theme.syntax_id() != self.opts.theme.syntax_id() {
                self.drop_tokens();
            }
        }
        if data_changed {
            self.reload_all();
        }
        if self.opts.syntax && (!old.syntax || theme_changed) {
            self.highlight_materialized(cx);
        }
        cx.notify();
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
                highlighting: &self.highlights,
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

    /// Lays out (synchronously) and starts loading the files within the
    /// materialization window.
    fn prepare_window(&mut self, height: f32, cx: &mut Context<Self>) {
        let range = self.doc.materialize_range(height, DEFAULT_WINDOW_SCREENS);
        for f in range {
            if self.doc.is_collapsed(f) {
                continue;
            }
            let change = &self.files[f as usize];
            if needs_blobs(change)
                && matches!(self.doc.state(f), FileState::Estimated | FileState::Evicted)
                && !self.loads.contains_key(&f)
            {
                self.start_load(f, cx);
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

    fn start_load(&mut self, f: u32, cx: &mut Context<Self>) {
        let generation = self.doc.begin_loading(f);
        let provider = self.provider.clone();
        let change = self.files[f as usize].clone();
        let (diff, words, layout) = (self.opts.diff, self.opts.word_diff, self.layout);
        let large = self.opts.large_file_changed_lines;
        let task = cx.spawn(async move |this, cx| {
            let built = cx
                .background_spawn(async move {
                    let file = MaterializedFile::load(&*provider, &change, &diff, words)?;
                    // Build the rows it will show here, off the main thread; a
                    // large diff shows a placeholder and needs none.
                    if file.diff.additions + file.diff.deletions <= large {
                        file.rows(layout);
                    }
                    Ok(file)
                })
                .await;
            this.update(cx, |v, cx| v.finish_load(f, generation, built, cx))
                .ok();
        });
        self.loads.insert(f, task);
    }

    fn finish_load(
        &mut self,
        f: u32,
        generation: u64,
        built: anyhow::Result<MaterializedFile>,
        cx: &mut Context<Self>,
    ) {
        self.loads.remove(&f);
        match built {
            Ok(file) => {
                let counts = SizeHint::Counts {
                    additions: file.diff.additions,
                    deletions: file.diff.deletions,
                    hunks: Some(file.diff.hunks.len() as u32),
                };
                let file = Arc::new(file);
                if !self.doc.set_materialized(f, generation, file.clone()) {
                    return;
                }
                self.doc.set_size_hint(f, counts);
                // New rows: lay the file out again on the next frame.
                self.layout_keys[f as usize] = None;
                if self.opts.syntax {
                    self.start_highlight(f, generation, file, cx);
                }
                for evicted in self.doc.evict_over_budget(DEFAULT_EVICTION_BUDGET_BYTES) {
                    self.layout_keys[evicted as usize] = None;
                }
            }
            Err(e) => {
                if !self.doc.set_failed(f, generation, format!("{e:#}")) {
                    return;
                }
                // Lay the error out on the next frame.
                self.layout_keys[f as usize] = None;
            }
        }
        cx.notify();
    }

    fn start_highlight(
        &mut self,
        f: u32,
        generation: u64,
        file: Arc<MaterializedFile>,
        cx: &mut Context<Self>,
    ) {
        let change = self.files[f as usize].clone();
        let highlighter = self.highlighter.clone();
        let theme = self.opts.theme.syntax_id();
        let task = cx.spawn(async move |this, cx| {
            let (old, new) = cx
                .background_spawn(async move {
                    let never = AtomicUsize::new(0);
                    let side = |path: Option<&polygloss_diff::GitPath>, text: &[u8]| {
                        highlight_side(&highlighter, path, text, &never)
                    };
                    (
                        side(change.old_path.as_ref(), &file.old_text),
                        side(change.new_path.as_ref(), &file.new_text),
                    )
                })
                .await;
            this.update(cx, |v, cx| {
                v.highlights.remove(&f);
                if theme != v.opts.theme.syntax_id() || (old.is_none() && new.is_none()) {
                    return;
                }
                let FileState::Materialized(current) = v.doc.state(f) else {
                    return;
                };
                let next = Arc::new(current.with_tokens(old, new));
                if v.doc.set_materialized(f, generation, next) {
                    cx.notify();
                }
            })
            .ok();
        });
        self.highlights.insert(f, task);
    }

    /// Highlights every materialized file that has no tokens yet.
    fn highlight_materialized(&mut self, cx: &mut Context<Self>) {
        for f in 0..self.doc.len() {
            if self.highlights.contains_key(&f) {
                continue;
            }
            if let FileState::Materialized(file) = self.doc.state(f)
                && file.old_tokens.is_none()
                && file.new_tokens.is_none()
            {
                let (generation, file) = (self.doc.generation(f), file.clone());
                self.start_highlight(f, generation, file, cx);
            }
        }
    }

    /// Drops tokens computed for another theme.
    fn drop_tokens(&mut self) {
        for f in 0..self.doc.len() {
            if let FileState::Materialized(file) = self.doc.state(f)
                && (file.old_tokens.is_some() || file.new_tokens.is_some())
            {
                let plain = Arc::new(file.with_tokens(None, None));
                let generation = self.doc.generation(f);
                self.doc.set_materialized(f, generation, plain);
            }
        }
    }

    /// Recomputes every loaded file (diff or word-diff options changed):
    /// files go back to unloaded (keeping their heights) and reload when near
    /// the viewport. Failed files are retried. Until a file's new data
    /// arrives its rows stay in place, blank, and a placeholder shows
    /// "Loading…" instead of its stale label (an error, a large-diff count).
    fn reload_all(&mut self) {
        self.loads.clear();
        self.highlights.clear();
        for f in 0..self.doc.len() {
            if matches!(
                self.doc.state(f),
                FileState::Materialized(_) | FileState::Loading { .. } | FileState::Failed(_)
            ) {
                self.doc.begin_loading(f);
                self.doc.cancel_loading(f);
                self.labels[f as usize] = None;
            }
        }
    }
}

impl Render for DiffViewport {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        DiffElement::new(cx.entity())
    }
}

/// Whether a file's body comes from its blobs (text and symlinks whose
/// content changed); binary, generated, submodule and content-equal changes
/// are drawn from metadata alone.
fn needs_blobs(change: &FileChange) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink)
        && !change.generated
        && change.old_blob != change.new_blob
}

/// Tokens for one side, or `None` (no text, no grammar, over budget, not
/// UTF-8).
fn highlight_side(
    highlighter: &Highlighter,
    path: Option<&polygloss_diff::GitPath>,
    text: &[u8],
    cancel: &AtomicUsize,
) -> Option<Arc<Tokens>> {
    if text.is_empty() {
        return None;
    }
    let lang = guess_language(&path?.text, &text[..text.len().min(1024)])?;
    highlighter
        .highlight(text, &lang, cancel, Budget::default())
        .ok()
        .map(Arc::new)
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
