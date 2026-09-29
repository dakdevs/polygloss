//! A frame's display list: which rows are visible and what each one paints
//! (design §11.6, §12.4 "Rows").
//!
//! Prepaint walks only the rows intersecting the viewport (O(log n) to find
//! the first one), shapes their text through the [`TextCache`] and records
//! quads and text in three layers (whole width, left half, right half) so
//! paint can replay them with one clip per layer, all quads before all text.
//! File headers go into a fourth layer painted after all of that, so the
//! header pinned at the top (design §11.6 "Sticky header") covers the rows
//! scrolling under it. Nothing here allocates per frame once the buffers are
//! warm, except for lines shaped for the first time and short header and
//! label strings.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Bounds, Font, Hsla, Pixels, Point, WindowTextSystem, point, px, size};
use polygloss_diff::rows::{Cell, Layout, LineKind, Row};
use polygloss_diff::{FileChange, Side};

use crate::controls::{Control, ControlAction, ControlLayer};
use crate::document::{BodyRow, Document, FileState};
use crate::file_flags::FileFlags;
use crate::gap::Gaps;
use crate::layout::{Columns, Geometry, Pane, digits};
use crate::materialize::MaterializedFile;
use crate::pipeline::Pipeline;
use crate::special::{BodyLabel, Specials};
use crate::style::{DiffStyle, ViewportTheme};
use crate::text_cache::{ShapedText, Shaper, TextCache, TextKey};

/// A filled rectangle with rounded corners and an optional 1 px border
/// (badges, checkboxes).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RoundedQuad {
    pub bounds: Bounds<Pixels>,
    pub background: Hsla,
    pub border: Option<Hsla>,
    pub radius: Pixels,
}

/// Quads and text drawn under one clip: plain quads, then rounded ones, then
/// text.
#[derive(Default)]
pub(crate) struct Layer {
    pub clip: Option<Bounds<Pixels>>,
    pub quads: Vec<(Bounds<Pixels>, Hsla)>,
    pub rounded: Vec<RoundedQuad>,
    pub texts: Vec<(Point<Pixels>, Rc<ShapedText>)>,
}

/// Layer indexes: the whole row, the left (old) half, the right (new) half,
/// and the file headers on top of them all.
pub(crate) const FULL: usize = 0;
pub(crate) const HEADERS: usize = 3;

/// Everything one frame paints, in window coordinates.
#[derive(Default)]
pub(crate) struct Frame {
    pub layers: [Layer; 4],
    /// The viewport's top-left corner.
    pub origin: Point<Pixels>,
    /// Clickable controls, in paint order.
    pub controls: Vec<Control>,
    /// Every painted header strip: it takes the clicks over the rows it
    /// covers.
    pub header_areas: Vec<Bounds<Pixels>>,
    /// Behind the control under the pointer.
    pub hover: Hsla,
    pub line_height: Pixels,
    /// Rows (headers included) intersecting the viewport.
    pub rows: u32,
    /// Lines shaped (cache misses) while building this frame, every pass
    /// included (set by the view).
    pub shaped: u32,
    /// Rows whose content is still loading ("Loading…" bodies).
    pub loading: u32,
    /// Code rows painted as plain text while their tokens are being computed,
    /// plus loading rows.
    pub unhighlighted: u32,
}

impl Frame {
    pub fn clear(&mut self) {
        for layer in &mut self.layers {
            layer.clip = None;
            layer.quads.clear();
            layer.rounded.clear();
            layer.texts.clear();
        }
        self.controls.clear();
        self.header_areas.clear();
        self.rows = 0;
        self.shaped = 0;
        self.loading = 0;
        self.unhighlighted = 0;
    }
}

/// What a visible row showed, for [`crate::ViewportDebug`] (formatted only
/// when asked).
#[cfg(feature = "debug-inspect")]
pub(crate) enum DebugContent {
    Header(Rc<ShapedText>),
    Label(Rc<ShapedText>),
    Unified {
        old: Option<u32>,
        new: Option<u32>,
        marker: char,
        text: Rc<ShapedText>,
    },
    Split {
        left: Option<(u32, char, Rc<ShapedText>)>,
        right: Option<(u32, char, Rc<ShapedText>)>,
    },
    Block(u64),
}

#[cfg(feature = "debug-inspect")]
pub(crate) struct DebugRow {
    pub y: f32,
    pub height: f32,
    pub styled: bool,
    pub content: DebugContent,
}

/// The body of a file whose data could not be loaded.
pub(crate) fn failed_label(message: &str) -> String {
    format!("Could not load this file: {message}")
}

/// The inputs of one frame, borrowed from the viewport.
pub(crate) struct Painter<'a> {
    pub doc: &'a Document,
    pub files: &'a [FileChange],
    pub file_labels: &'a [Option<BodyLabel>],
    /// Host-owned review state per file (header checkbox and badges).
    pub flags: &'a [FileFlags],
    /// Revealed context and the rows built with it.
    pub gaps: &'a Gaps,
    /// Blob sizes, LFS pointers, "Load diff" requests.
    pub special: &'a Specials,
    pub theme: &'a ViewportTheme,
    pub style: DiffStyle,
    pub syntax: bool,
    pub word_diff: bool,
    pub geometry: Geometry,
    pub font: &'a Font,
    pub layout: Layout,
    pub bounds: Bounds<Pixels>,
    /// `scroll_top` snapped to device pixels.
    pub scroll_top: f64,
    pub cache: &'a mut TextCache,
    /// Which sides still wait for syntax tokens.
    pub pipeline: &'a Pipeline,
    pub text_system: Arc<WindowTextSystem>,
    pub frame: &'a mut Frame,
    /// Rows whose measured wrapped height differs from the layout:
    /// `(file, row, height)`.
    pub corrections: Vec<(u32, u32, f32)>,
    #[cfg(feature = "debug-inspect")]
    pub debug: &'a mut Vec<DebugRow>,
    #[cfg(feature = "debug-inspect")]
    pub debug_headers: &'a mut Vec<crate::debug::HeaderDebug>,
    /// Every text queued, `(x, y, text)` relative to the viewport.
    #[cfg(feature = "debug-inspect")]
    pub debug_text: &'a mut Vec<(f32, f32, Rc<ShapedText>)>,
}

impl Painter<'_> {
    /// Fills the frame with every row intersecting the viewport.
    pub fn paint_visible(&mut self) {
        let b = self.bounds;
        let (width, height) = (b.size.width.as_f32(), b.size.height.as_f32());
        self.frame.line_height = px(self.geometry.row_height);
        self.frame.origin = b.origin;
        self.frame.hover = self.theme.hover;
        self.frame.layers[FULL].clip = Some(b);
        self.frame.layers[HEADERS].clip = Some(b);
        self.quad(FULL, 0.0, 0.0, width, height, self.theme.background);
        if self.layout == Layout::Split {
            let half = (width / 2.0).floor();
            self.frame.layers[1].clip = Some(Bounds::new(b.origin, size(px(half), b.size.height)));
            self.frame.layers[2].clip = Some(Bounds::new(
                point(b.origin.x + px(half), b.origin.y),
                size(px(width - half), b.size.height),
            ));
        }
        let header_h = self.doc.metrics().header_height;
        let visible = self.doc.visible(height);
        for f in visible.clone() {
            let top = (self.doc.file_top(f) - self.scroll_top) as f32;
            // The first file's header pins at the top while its body scrolls
            // under it, until the next file's header pushes it up.
            let y = if f == visible.start && top < 0.0 {
                (top + self.doc.file_height(f) - header_h).min(0.0)
            } else {
                top
            };
            if y + header_h > 0.0 {
                self.header(f, y, header_h, y != top);
            }
            if self.doc.is_collapsed(f) {
                continue;
            }
            let body_top = top + header_h;
            match self.doc.file_layout(f) {
                Some(layout) => {
                    let skip = f64::from((-body_top).max(0.0));
                    let (first, _) = layout.row_at(skip);
                    let mut y = body_top + layout.row_top(first) as f32;
                    for i in first..layout.len() {
                        if y >= height {
                            break;
                        }
                        let h = layout.row_height(i);
                        if y + h > 0.0 {
                            self.body_row(f, i as u32, layout.rows()[i], y, h);
                        }
                        y += h;
                    }
                }
                None => {
                    // Not laid out yet. The view lays out every file in the
                    // materialization window before painting (a failed file
                    // gets its error as a placeholder), so this is a body
                    // whose data is on its way.
                    let label = match self.doc.state(f) {
                        FileState::Failed(msg) => failed_label(msg),
                        _ => {
                            self.count_loading();
                            "Loading…".to_owned()
                        }
                    };
                    let h = self.doc.file_height(f) - header_h;
                    self.label_row(f, &label, body_top, h.min(self.geometry.row_height * 2.0));
                }
            }
        }
    }

    pub(crate) fn materialized(&self, f: u32) -> Option<&Arc<MaterializedFile>> {
        match self.doc.state(f) {
            FileState::Materialized(file) => Some(file),
            _ => None,
        }
    }

    pub(crate) fn columns(&self, file: Option<&MaterializedFile>) -> Columns {
        let lines = file.map_or(0, |m| m.diff.old.len().max(m.diff.new.len()));
        Columns::new(
            self.layout,
            self.bounds.size.width.as_f32(),
            self.geometry.advance,
            digits(lines),
            self.style.indicators,
        )
    }

    fn body_row(&mut self, f: u32, i: u32, row: BodyRow, y: f32, h: f32) {
        self.frame.rows += 1;
        match row {
            BodyRow::Line { diff_row, .. } => {
                let Some(file) = self.materialized(f).cloned() else {
                    // Reloading (options changed): rows keep their place,
                    // blank until the new data arrives. (A failed reload
                    // replaces them with its error before painting.)
                    if !matches!(self.doc.state(f), FileState::Failed(_)) {
                        self.count_loading();
                    }
                    return;
                };
                match self
                    .gaps
                    .painted_rows(f, &file, self.layout)
                    .get(diff_row as usize)
                {
                    Some(Row::Unified {
                        old,
                        new,
                        kind,
                        pair,
                    }) => self.unified_line(f, i, &file, y, h, *old, *new, *kind, pair.is_some()),
                    Some(Row::Split { left, right }) => {
                        self.split_line(f, i, &file, y, h, *left, *right)
                    }
                    _ => {}
                }
            }
            BodyRow::Gap { .. } => self.gap_row(f, row, y, h),
            BodyRow::NoNewline { side, .. } => {
                let cols = self.columns(self.materialized(f).map(|m| &**m));
                let pane = cols.code_pane(side);
                let text = self.label("\\ No newline at end of file", 1, self.theme.muted);
                let layer = pane_layer(pane);
                self.text(layer, cols.code_x(pane), y, text.clone());
                #[cfg(feature = "debug-inspect")]
                self.debug.push(DebugRow {
                    y,
                    height: h,
                    styled: false,
                    content: DebugContent::Label(text),
                });
            }
            BodyRow::Block(_id) => {
                #[cfg(feature = "debug-inspect")]
                self.debug.push(DebugRow {
                    y,
                    height: h,
                    styled: false,
                    content: DebugContent::Block(_id.0),
                });
            }
            BodyRow::Placeholder => self.placeholder_row(f, y, h),
        }
    }

    /// A muted label row (gaps, placeholders) at the code column; returns
    /// the right edge of the label, for controls that follow it.
    pub(crate) fn label_at(&mut self, f: u32, label: &str, y: f32, h: f32) -> f32 {
        let cols = self.columns(self.materialized(f).map(|m| &**m));
        let pane = match self.layout {
            Layout::Split => Pane::Half(0),
            Layout::Unified => Pane::Full,
        };
        let text = self.label(label, 1, self.theme.muted);
        let row_h = self.geometry.row_height;
        let x = cols.indicator_x(pane);
        self.text(FULL, x, y + ((h - row_h) / 2.0).max(0.0), text.clone());
        let right = x + text.shaped.width();
        #[cfg(feature = "debug-inspect")]
        self.debug.push(DebugRow {
            y,
            height: h,
            styled: false,
            content: DebugContent::Label(text),
        });
        right
    }

    /// A label in place of a body that has no rows yet.
    fn label_row(&mut self, f: u32, label: &str, y: f32, h: f32) {
        self.frame.rows += 1;
        self.label_at(f, label, y, h);
    }

    /// A clickable text control after a label: `text` in the accent color,
    /// starting at `x`, centered in the row `y..y + h`. Returns its right
    /// edge.
    pub(crate) fn link(
        &mut self,
        action: ControlAction,
        text: &str,
        x: f32,
        y: f32,
        h: f32,
    ) -> f32 {
        let a = self.geometry.advance;
        let row_h = self.geometry.row_height;
        let shaped = self.label(text, crate::header::SLOT_ACCENT, self.theme.accent);
        let w = shaped.shaped.width() + a;
        self.text(FULL, x + 0.5 * a, y + ((h - row_h) / 2.0).max(0.0), shaped);
        let pad = ((h - row_h) / 2.0).clamp(0.0, 4.0);
        self.control(action, ControlLayer::Body, x, y + pad, w, h - 2.0 * pad);
        x + w
    }

    #[allow(clippy::too_many_arguments)]
    fn unified_line(
        &mut self,
        f: u32,
        i: u32,
        file: &MaterializedFile,
        y: f32,
        h: f32,
        old: Option<u32>,
        new: Option<u32>,
        kind: LineKind,
        paired: bool,
    ) {
        let cols = self.columns(Some(file));
        let width = cols.width;
        if self.style.backgrounds
            && let Some(bg) = self.kind_background(kind)
        {
            self.quad(FULL, 0.0, y, width, h, bg);
        }
        if let Some(o) = old {
            self.number(FULL, o + 1, cols.number_right(Pane::Full, 0), y);
        }
        if let Some(n) = new {
            self.number(FULL, n + 1, cols.number_right(Pane::Full, 1), y);
        }
        self.indicator(FULL, kind, cols.indicator_x(Pane::Full), y, h);
        let (side, line) = match (kind, old, new) {
            (LineKind::Removed, Some(o), _) => (Side::Old, o),
            (_, _, Some(n)) => (Side::New, n),
            (_, Some(o), None) => (Side::Old, o),
            _ => return,
        };
        let (text, rows) = self.code(f, file, side, line, &cols, Pane::Full, y, paired);
        self.measure(f, i, h, rows);
        self.count_unhighlighted(f, file, &[side]);
        #[cfg(feature = "debug-inspect")]
        self.debug.push(DebugRow {
            y,
            height: h,
            styled: self.syntax && file.tokens(side).is_some(),
            content: DebugContent::Unified {
                old: old.map(|o| o + 1),
                new: new.map(|n| n + 1),
                marker: marker(kind),
                text,
            },
        });
        #[cfg(not(feature = "debug-inspect"))]
        let _ = text;
    }

    #[allow(clippy::too_many_arguments)]
    fn split_line(
        &mut self,
        f: u32,
        i: u32,
        file: &MaterializedFile,
        y: f32,
        h: f32,
        left: Option<Cell>,
        right: Option<Cell>,
    ) {
        let cols = self.columns(Some(file));
        let mut rows = 1;
        let mut cells: [Option<(u32, char, Rc<ShapedText>)>; 2] = [None, None];
        for (k, cell) in [left, right].into_iter().enumerate() {
            let pane = Pane::Half(k as u8);
            let layer = pane_layer(pane);
            let (x, w) = cols.pane(pane);
            let Some(cell) = cell else {
                self.quad(layer, x, y, w, h, self.theme.empty_cell);
                continue;
            };
            if self.style.backgrounds
                && let Some(bg) = self.kind_background(cell.kind)
            {
                self.quad(layer, x, y, w, h, bg);
            }
            self.number(layer, cell.line + 1, cols.number_right(pane, 0), y);
            self.indicator(layer, cell.kind, cols.indicator_x(pane), y, h);
            let side = if k == 0 { Side::Old } else { Side::New };
            let paired = cell.pair.is_some();
            let (text, r) = self.code(f, file, side, cell.line, &cols, pane, y, paired);
            rows = rows.max(r);
            cells[k] = Some((cell.line + 1, marker(cell.kind), text));
        }
        // The divider is the left half's last pixel column, drawn after its
        // background (the right half's quads start at `half`).
        self.quad(1, cols.half - 1.0, y, 1.0, h, self.theme.border);
        self.measure(f, i, h, rows);
        let sides = match (left, right) {
            (Some(_), Some(_)) => &[Side::Old, Side::New][..],
            (Some(_), None) => &[Side::Old][..],
            (None, _) => &[Side::New][..],
        };
        self.count_unhighlighted(f, file, sides);
        #[cfg(feature = "debug-inspect")]
        {
            let [l, r] = cells;
            let styled = self.syntax
                && (left.is_some() && file.tokens(Side::Old).is_some()
                    || right.is_some() && file.tokens(Side::New).is_some());
            self.debug.push(DebugRow {
                y,
                height: h,
                styled,
                content: DebugContent::Split { left: l, right: r },
            });
        }
        #[cfg(not(feature = "debug-inspect"))]
        let _ = cells;
    }

    /// Records a clickable control at viewport-relative `(x, y)`.
    pub(crate) fn control(
        &mut self,
        action: ControlAction,
        layer: ControlLayer,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) {
        self.frame.controls.push(Control {
            action,
            bounds: self.bounds_at(x, y, w, h),
            layer,
            run: None,
        });
    }

    /// Viewport-relative `(x, y, w, h)` in window coordinates.
    pub(crate) fn bounds_at(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        let o = self.bounds.origin;
        Bounds::new(point(o.x + px(x), o.y + px(y)), size(px(w), px(h)))
    }

    /// Queues a rounded rectangle at viewport-relative coordinates.
    pub(crate) fn rounded(
        &mut self,
        layer: usize,
        (x, y, w, h): (f32, f32, f32, f32),
        background: Hsla,
        border: Option<Hsla>,
        radius: f32,
    ) {
        let bounds = self.bounds_at(x, y, w, h);
        self.frame.layers[layer].rounded.push(RoundedQuad {
            bounds,
            background,
            border,
            radius: px(radius),
        });
    }

    fn kind_background(&self, kind: LineKind) -> Option<Hsla> {
        match kind {
            LineKind::Removed => Some(self.theme.removed_background),
            LineKind::Added => Some(self.theme.added_background),
            LineKind::Context => None,
        }
    }

    /// Counts a visible row whose file's data is still on its way (it is
    /// unhighlighted too).
    pub(crate) fn count_loading(&mut self) {
        self.frame.loading += 1;
        self.frame.unhighlighted += 1;
    }

    /// Counts a code row showing `sides` as unhighlighted when one of them has
    /// no tokens yet but may still get them (not when it never will: no
    /// grammar, over 100k lines, over its time budget).
    fn count_unhighlighted(&mut self, f: u32, file: &MaterializedFile, sides: &[Side]) {
        if self.syntax
            && sides
                .iter()
                .any(|&s| file.tokens(s).is_none() && self.pipeline.tokens_pending(f, s))
        {
            self.frame.unhighlighted += 1;
        }
    }

    /// Records a wrapped row whose shaped height differs from its layout.
    fn measure(&mut self, f: u32, i: u32, h: f32, visual_rows: u32) {
        if self.style.wrap {
            let want = visual_rows.max(1) as f32 * self.geometry.row_height;
            if want != h {
                self.corrections.push((f, i, want));
            }
        }
    }

    /// Shapes (or reuses) line `line` of `side`, queues it and its word
    /// highlights, and returns it with its visual row count.
    #[allow(clippy::too_many_arguments)]
    fn code(
        &mut self,
        f: u32,
        file: &MaterializedFile,
        side: Side,
        line: u32,
        cols: &Columns,
        pane: Pane,
        y: f32,
        paired: bool,
    ) -> (Rc<ShapedText>, u32) {
        let tokens = if self.syntax { file.tokens(side) } else { None };
        let wrap_width = if self.style.wrap {
            cols.code_width(pane).floor()
        } else {
            0.0
        };
        let key = TextKey::Code {
            file: f,
            side,
            line,
            styled: tokens.is_some(),
            wrap: wrap_width as u32,
        };
        let shaper = Shaper {
            theme: self.theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        let words = paired && self.word_diff;
        let shaped = self.cache.get_or_shape(key, || {
            shaper.code(file, side, line, tokens.map(|t| &**t), wrap_width, words)
        });
        let layer = pane_layer(pane);
        let x = cols.code_x(pane);
        let row_h = self.geometry.row_height;
        let color = match side {
            Side::Old => self.theme.removed_word,
            Side::New => self.theme.added_word,
        };
        for w in &shaped.words {
            self.quad(
                layer,
                x + w.x0,
                y + w.row as f32 * row_h,
                (w.x1 - w.x0).max(0.0),
                row_h,
                color,
            );
        }
        self.text(layer, x, y, shaped.clone());
        let rows = shaped.shaped.visual_rows();
        (shaped, rows)
    }

    /// A single-line label in one color, cached by content.
    pub(crate) fn label(&mut self, text: &str, slot: u8, color: Hsla) -> Rc<ShapedText> {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = TextKey::Label {
            hash: hasher.finish(),
            color: slot,
        };
        let shaper = Shaper {
            theme: self.theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        self.cache.get_or_shape(key, || shaper.label(text, color))
    }

    /// Queues a quad at viewport-relative coordinates.
    pub(crate) fn quad(&mut self, layer: usize, x: f32, y: f32, w: f32, h: f32, color: Hsla) {
        let bounds = self.bounds_at(x, y, w, h);
        self.frame.layers[layer].quads.push((bounds, color));
    }

    /// Queues text with its top-left corner at viewport-relative `(x, y)`.
    pub(crate) fn text(&mut self, layer: usize, x: f32, y: f32, text: Rc<ShapedText>) {
        #[cfg(feature = "debug-inspect")]
        self.debug_text.push((x, y, text.clone()));
        let o = self.bounds.origin;
        self.frame.layers[layer]
            .texts
            .push((point(o.x + px(x), o.y + px(y)), text));
    }
}

fn marker(kind: LineKind) -> char {
    match kind {
        LineKind::Context => ' ',
        LineKind::Removed => '-',
        LineKind::Added => '+',
    }
}

pub(crate) fn pane_layer(pane: Pane) -> usize {
    match pane {
        Pane::Full => FULL,
        Pane::Half(0) => 1,
        Pane::Half(_) => 2,
    }
}
