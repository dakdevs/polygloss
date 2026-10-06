//! A frame's display list: which rows are visible and what each one paints
//! (design §11.6, §12.4 "Rows").
//!
//! A one-sided file (added or deleted text) is drawn as one pane in both
//! layouts: its unified rows, with one number column
//! ([`crate::layout::layout_for`], [`Columns::for_file`]).
//!
//! Prepaint walks only the rows intersecting the viewport (O(log n) to find
//! the first one), shapes their text through the [`TextCache`] and records
//! quads and text in three layers (whole width, left half, right half) so
//! paint can replay them with one clip per layer, all quads before all text.
//! The canvas and the file cards go into a layer painted before them
//! ([`crate::card`]); rows fill a card's inner width, so nothing paints over
//! its border. File headers go into a fourth layer painted after all of that,
//! so the header pinned at the top (design §11.6 "Sticky header") covers the
//! rows scrolling under it. Nothing here allocates per frame once the
//! buffers are warm, except for lines shaped for the first time and short
//! header and label strings.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Bounds, Corners, Edges, Font, Hsla, Pixels, Point, SharedString, WindowTextSystem, point, px,
    size,
};
use polygloss_diff::rows::{Cell, Layout, LineKind, Row};
use polygloss_diff::{FileChange, Side};

use crate::blocks::{BlockSlot, Blocks, RenderBlock};
use crate::card::{CardStyle, PreludeSlot};
use crate::controls::{Control, ControlAction, ControlLayer};
use crate::cursor::CursorPos;
use crate::document::{BodyRow, Document, FileState};
use crate::file_flags::FileFlags;
use crate::find::FindState;
use crate::gap::Gaps;
use crate::layout::{Columns, Geometry, Pane, digits, layout_for};
use crate::materialize::MaterializedFile;
use crate::pipeline::Pipeline;
use crate::reveal::RevealGeom;
use crate::section_band::Band;
use crate::selection::{PlusHit, TextSelection};
use crate::space::{gap, height, radius, stroke};
use crate::special::{BodyLabel, Specials};
use crate::style::{DiffStyle, ViewportTheme};
use crate::text_cache::{FONT_CODE, FONT_UI, NumberKind, ShapedText, Shaper, TextCache, TextKey};

/// A filled rectangle with rounded corners and an optional border (cards,
/// headers, badges, checkboxes).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RoundedQuad {
    pub bounds: Bounds<Pixels>,
    pub background: Hsla,
    pub border: Option<Hsla>,
    /// Used with `border` only.
    pub border_widths: Edges<Pixels>,
    pub radius: Corners<Pixels>,
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
/// the same three for a revealing body's rows ([`REVEAL`] on, each also cut
/// at the curtain), and the file headers on top of them all.
pub(crate) const FULL: usize = 0;
pub(crate) const REVEAL: usize = 3;
pub(crate) const HEADERS: usize = 6;

/// Everything one frame paints, in window coordinates.
#[derive(Default)]
pub(crate) struct Frame {
    /// The canvas and the cards, under everything else.
    pub cards: Layer,
    pub layers: [Layer; 7],
    /// The viewport's top-left corner.
    pub origin: Point<Pixels>,
    /// Clickable controls, in paint order.
    pub controls: Vec<Control>,
    /// Every painted header strip: it takes the clicks over the rows it
    /// covers.
    pub header_areas: Vec<Bounds<Pixels>>,
    /// Every painted card's whole outer bounds, borders included (its quad
    /// is cut near the viewport's edges), top to bottom.
    pub card_bounds: Vec<(u32, Bounds<Pixels>)>,
    /// Over the control under the pointer.
    pub hover: Hsla,
    /// Over the pressed control while the pointer is on it.
    pub pressed: Hsla,
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
    /// Visible host blocks, top to bottom (their elements are rendered,
    /// measured and painted by the element, see [`crate::blocks`]).
    pub blocks: Vec<BlockSlot>,
    /// The prelude, when it reaches into the viewport (rendered, measured
    /// and painted like a block).
    pub prelude: Option<PreludeSlot>,
    /// Every painted code cell (a unified row, or one half of a split row),
    /// for mouse hit tests ([`crate::selection`]).
    pub cells: Vec<LineCell>,
    /// The "+" painted on the hovered line numbers.
    pub plus: Option<PlusHit>,
    /// SVG icons (`icons/<name>.svg`) in window coordinates and their
    /// rotation in degrees about their centre (a turning chevron), painted
    /// after the header layer's quads and before its text.
    pub icons: Vec<(Bounds<Pixels>, SharedString, Hsla, f32)>,
    /// A Reduced fade's veil over a revealing body's rows: the card's
    /// background at the share of the rows not yet (or no longer) shown,
    /// painted over the rows and blocks, under the headers.
    pub veil: Option<(Bounds<Pixels>, Hsla)>,
    /// Each painted file's card top, viewport-relative.
    #[cfg(feature = "debug-inspect")]
    pub slots: Vec<(u32, f32)>,
    /// Each painted header's chevron angle, in degrees.
    #[cfg(feature = "debug-inspect")]
    pub chevrons: Vec<(u32, f32)>,
    /// The running reveal as painted.
    #[cfg(feature = "debug-inspect")]
    pub reveal: Option<crate::debug::RevealDebug>,
}

/// A painted code cell, viewport-relative: the gutter (numbers and
/// indicator) is `x..code_x`, the code `code_x..right`.
pub(crate) struct LineCell {
    pub file_idx: u32,
    /// The lines the cell shows (a unified context row shows both).
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// The side whose text is painted (and whose line the gutter targets).
    pub side: Side,
    pub x: f32,
    pub code_x: f32,
    pub right: f32,
    pub y: f32,
    pub h: f32,
    pub text: Rc<ShapedText>,
}

/// What a frame marks on its rows: the cursor or range, the selected text
/// and, for the "+", the pointer (viewport-relative) while over the
/// viewport.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Marks {
    pub cursor: Option<CursorPos>,
    pub selection: Option<TextSelection>,
    pub pointer: Option<(f32, f32)>,
    /// A text drag is in progress (no "+" then).
    pub text_drag: bool,
    /// Old-side lines get a "+" ([`crate::DiffViewport::set_old_side_comments`]).
    pub old_side_comments: bool,
}

impl Frame {
    pub fn clear(&mut self) {
        for layer in std::iter::once(&mut self.cards).chain(&mut self.layers) {
            layer.clip = None;
            layer.quads.clear();
            layer.rounded.clear();
            layer.texts.clear();
        }
        self.controls.clear();
        self.header_areas.clear();
        self.card_bounds.clear();
        self.blocks.clear();
        self.prelude = None;
        self.cells.clear();
        self.plus = None;
        self.icons.clear();
        self.veil = None;
        #[cfg(feature = "debug-inspect")]
        {
            self.slots.clear();
            self.chevrons.clear();
            self.reveal = None;
        }
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
    /// Labels painted side by side on one row (split `\ No newline` markers).
    Labels(Vec<Rc<ShapedText>>),
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
    /// Two blocks side by side (split): the old side's, then the new side's.
    BlockPair(u64, u64),
    /// A section band: `▸` (closed) or `▾` (open) and its label.
    Band(String),
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
    /// The code font.
    pub font: &'a Font,
    /// The UI font (pills, the Viewed label).
    pub ui_font: &'a Font,
    pub layout: Layout,
    /// The viewport, in window coordinates.
    pub bounds: Bounds<Pixels>,
    /// Where rows go: `bounds` inset by a card's margin and border on the
    /// left and right (`bounds` in the flat layout).
    pub inner: Bounds<Pixels>,
    /// The rows' width that wrapping and host blocks go by: `inner`'s, or a
    /// held one ([`crate::DiffViewport::hold_layout`]).
    pub layout_width: f32,
    /// The width the prelude is measured and laid out at.
    pub prelude_width: f32,
    pub cards: Option<CardStyle>,
    /// The host's prelude.
    pub prelude: Option<&'a RenderBlock>,
    /// `scroll_top` snapped to device pixels.
    pub scroll_top: f64,
    /// The running reveal, placed for this frame ([`crate::reveal`]).
    pub reveal: Option<RevealGeom>,
    /// Painting the revealing body's rows: row layers map to their
    /// [`REVEAL`] twins.
    pub in_reveal: bool,
    /// While painting the revealing body: where its rows show, in window
    /// coordinates (host blocks are cut to it too).
    pub rows_clip: Option<Bounds<Pixels>>,
    pub cache: &'a mut TextCache,
    /// Which sides still wait for syntax tokens.
    pub pipeline: &'a Pipeline,
    /// Host blocks, for their columns and render functions.
    pub blocks: &'a Blocks,
    /// What each section's band shows, in the document's section order.
    pub bands: &'a [Band],
    /// Find matches to mark in the code.
    pub find: Option<&'a mut FindState>,
    pub text_system: Arc<WindowTextSystem>,
    /// Cursor, range, selection and pointer.
    pub marks: Marks,
    pub frame: &'a mut Frame,
    /// Rows whose measured wrapped height differs from the layout:
    /// `(file, row, height)`.
    pub corrections: Vec<(u32, u32, f32)>,
    #[cfg(feature = "debug-inspect")]
    pub debug: &'a mut Vec<DebugRow>,
    #[cfg(feature = "debug-inspect")]
    pub debug_headers: &'a mut Vec<crate::debug::HeaderDebug>,
    #[cfg(feature = "debug-inspect")]
    pub debug_bands: &'a mut Vec<crate::debug::BandDebug>,
    /// Every text queued, `(x, y, text)` relative to the viewport.
    #[cfg(feature = "debug-inspect")]
    pub debug_text: &'a mut Vec<(f32, f32, Rc<ShapedText>)>,
}

impl Painter<'_> {
    /// Fills the frame with every row intersecting the viewport.
    pub fn paint_visible(&mut self) {
        let b = self.bounds;
        let height = b.size.height.as_f32();
        self.frame.line_height = px(self.geometry.row_height);
        self.frame.origin = b.origin;
        self.frame.hover = self.theme.hover;
        self.frame.pressed = self.theme.pressed;
        // Rows never paint over a card's border or the canvas beside it.
        self.frame.layers[FULL].clip = Some(self.inner);
        self.frame.layers[HEADERS].clip = Some(b);
        // A body opening moves the slots below it up from where the model
        // has them: those further down come into view.
        let lifted = self.reveal.map_or(0.0, |r| (-r.shift).max(0.0));
        let visible = self.doc.visible(height + lifted);
        self.paint_canvas(visible);
        if self.layout == Layout::Split {
            let [left, right] = self.halves(self.inner);
            self.frame.layers[1].clip = Some(left);
            self.frame.layers[2].clip = Some(right);
        }
        #[cfg(feature = "debug-inspect")]
        {
            self.frame.reveal = self.reveal.map(|r| crate::debug::RevealDebug {
                file_idx: r.file,
                slot: r.slot,
                height: r.frame_bottom - r.body_top,
                curtain: r.curtain,
                opacity: r.opacity,
                frozen: r.frozen,
            });
        }
        self.place_prelude();
        let header_h = self.doc.metrics().header_height;
        let doc = self.doc;
        let mut first = true;
        // Bands in visible slots, painted in order with the files: a band
        // comes before the file whose lead holds it.
        let mut bands = doc.sections_in(visible).peekable();
        for f in doc.shown_files(visible) {
            let slot = doc.slot(f);
            while let Some(s) = bands.next_if(|&s| doc.band_slot(s) <= slot) {
                self.paint_band(s, height);
            }
            let top = (self.doc.header_top(f) - self.scroll_top) as f32 + self.shift(slot);
            if top >= height {
                // Only its lead shows (the canvas above its card, or the
                // prelude): nothing of the card is in view.
                break;
            }
            #[cfg(feature = "debug-inspect")]
            self.frame.slots.push((f, top));
            // The first shown file's header pins at the top while its body
            // scrolls under it, until its body's end (the next card in the
            // flat layout) pushes it up.
            let pins = std::mem::take(&mut first);
            let y = if pins && top < 0.0 {
                painted_header_y(doc, f, self.scroll_top)
            } else {
                top
            };
            if y + header_h > 0.0 {
                self.header(f, y, header_h, y != top);
            }
            if let Some(reveal) = self.reveal.filter(|r| r.file == f) {
                self.reveal_body(reveal, height);
                continue;
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
                    // Centered as a placeholder's would be, in the body's
                    // first placeholder height.
                    let h = self.doc.body_height(f);
                    let placeholder = self.doc.metrics().placeholder_height;
                    self.label_row(f, &label, body_top, h.min(placeholder));
                }
            }
        }
        for s in bands {
            self.paint_band(s, height);
        }
    }

    /// The left and right halves of `rows` (split).
    fn halves(&self, rows: Bounds<Pixels>) -> [Bounds<Pixels>; 2] {
        let half = (rows.size.width.as_f32() / 2.0).floor();
        [
            Bounds::new(rows.origin, size(px(half), rows.size.height)),
            Bounds::new(
                point(rows.origin.x + px(half), rows.origin.y),
                size(rows.size.width - px(half), rows.size.height),
            ),
        ]
    }

    /// How far a slot is painted from where the model has it: by the running
    /// reveal's frame, for every slot after its body's.
    pub(crate) fn shift(&self, slot: u32) -> f32 {
        match self.reveal {
            Some(r) if slot > r.slot => r.shift,
            _ => 0.0,
        }
    }

    /// The revealing body (ADR-0030 M3): its rows at the screen y they had
    /// before the commit, cut at the curtain into the [`REVEAL`] layers (and
    /// its hit targets with them), and a Reduced fade's veil over them. A
    /// closing body is closed in the model from the commit, so its rows take
    /// no clicks or hover (rule 9: what leaves is inert).
    fn reveal_body(&mut self, reveal: RevealGeom, height: f32) {
        let f = reveal.file;
        let (lo, hi) = (reveal.body_top.max(0.0), reveal.curtain.min(height));
        let Some(layout) = self.doc.file_layout(f).filter(|_| hi > lo) else {
            return;
        };
        let band = |rows: Bounds<Pixels>| {
            let o = self.bounds.origin;
            Bounds::new(
                point(rows.origin.x, o.y + px(lo)),
                size(rows.size.width, px(hi - lo)),
            )
        };
        let shown = band(self.inner);
        self.frame.layers[REVEAL + FULL].clip = Some(shown);
        if self.layout == Layout::Split {
            let [left, right] = self.halves(self.inner);
            self.frame.layers[REVEAL + 1].clip = Some(band(left));
            self.frame.layers[REVEAL + 2].clip = Some(band(right));
        }
        let (cells, controls) = (self.frame.cells.len(), self.frame.controls.len());
        let closing = self.doc.is_collapsed(f);
        let pointer = self.marks.pointer;
        if closing {
            self.marks.pointer = None;
        }
        self.in_reveal = true;
        self.rows_clip = Some(shown);
        let skip = f64::from(lo - reveal.rows_y).max(0.0);
        let (first, _) = layout.row_at(skip);
        let mut y = reveal.rows_y + layout.row_top(first) as f32;
        for i in first..layout.len() {
            if y >= hi {
                break;
            }
            let h = layout.row_height(i);
            if y + h > lo {
                self.body_row(f, i as u32, layout.rows()[i], y, h);
            }
            y += h;
        }
        self.in_reveal = false;
        self.rows_clip = None;
        self.marks.pointer = pointer;
        if closing {
            self.frame.cells.truncate(cells);
            self.frame.controls.truncate(controls);
        } else {
            // What is cut away takes no clicks.
            for c in &mut self.frame.cells[cells..] {
                c.h = c.h.min(hi - c.y);
            }
            self.frame.cells.retain(|c| c.h > 0.0);
            for c in &mut self.frame.controls[controls..] {
                c.bounds = c.bounds.intersect(&shown);
            }
        }
        if reveal.opacity < 1.0 {
            let background = match self.cards {
                Some(_) => self.theme.card_background,
                None => self.theme.background,
            };
            let veil = background.opacity(1.0 - reveal.opacity);
            self.frame.veil = Some((shown, veil));
        }
    }

    /// Section `s`'s band, when it reaches into the viewport (`height` tall).
    fn paint_band(&mut self, s: usize, height: f32) {
        let y = (self.doc.band_top(s) - self.scroll_top) as f32 + self.shift(self.doc.band_slot(s));
        if y < height && y + self.doc.metrics().band_height > 0.0 {
            self.band(s, y);
        }
    }

    pub(crate) fn materialized(&self, f: u32) -> Option<&Arc<MaterializedFile>> {
        match self.doc.state(f) {
            FileState::Materialized(file) => Some(file),
            _ => None,
        }
    }

    /// The columns of file `f`'s rows ([`Columns::for_file`]).
    pub(crate) fn columns(&self, f: u32, file: Option<&MaterializedFile>) -> Columns {
        let lines = file.map_or(0, |m| m.diff.old.len().max(m.diff.new.len()));
        let (x, width) = self.inner_x_w();
        Columns::new(
            self.layout,
            x,
            width,
            self.geometry.advance,
            digits(lines),
            self.style.indicators,
        )
        .for_file(&self.files[f as usize])
    }

    /// The layout file `f` is drawn in ([`layout_for`]).
    pub(crate) fn file_layout(&self, f: u32) -> Layout {
        layout_for(&self.files[f as usize], self.layout)
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
                    .painted_rows(f, &file, self.file_layout(f))
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
            BodyRow::NoNewline { side, .. } => self.no_newline(f, &[side], y, h),
            BodyRow::NoNewlineBoth { .. } => self.no_newline(f, &[Side::Old, Side::New], y, h),
            BodyRow::Block(id) => self.block(f, id, y, h),
            BodyRow::BlockPair { old, new } => self.block_pair(f, old, new, y, h),
            BodyRow::Placeholder => self.placeholder_row(f, y, h),
        }
    }

    /// `\ No newline at end of file` at the code column of each of `sides`
    /// (one row; in split, each marker in its own half).
    fn no_newline(&mut self, f: u32, sides: &[Side], y: f32, h: f32) {
        let cols = self.columns(f, self.materialized(f).map(|m| &**m));
        let text = self.label("\\ No newline at end of file", 1, self.theme.muted);
        for &side in sides {
            let pane = cols.code_pane(side);
            self.text(pane_layer(pane), cols.code_x(pane), y, text.clone());
        }
        #[cfg(feature = "debug-inspect")]
        self.debug.push(DebugRow {
            y,
            height: h,
            styled: false,
            content: DebugContent::Labels(vec![text; sides.len()]),
        });
        #[cfg(not(feature = "debug-inspect"))]
        let _ = h;
    }

    /// A muted label row (gaps, placeholders) at the code column, centered
    /// in the row `y..y + h`; returns the right edge of the label, for the
    /// links that follow it.
    pub(crate) fn label_at(&mut self, f: u32, label: &str, y: f32, h: f32) -> f32 {
        let cols = self.columns(f, self.materialized(f).map(|m| &**m));
        let pane = match cols.layout {
            Layout::Split => Pane::Half(0),
            Layout::Unified => Pane::Full,
        };
        let text = self.label(label, 1, self.theme.muted);
        let row_h = self.geometry.row_height;
        let x = cols.code_x(pane);
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

    /// A text link (ADR-0031 C2): `text`'s box starts at `x`, half a
    /// `gap::GROUP` wider than the text on each side, so neighbouring links'
    /// boxes meet and their texts stand `GROUP` apart, and `height::SM`
    /// tall, centered in the row `y..y + h`. The box is its control, and
    /// where its hover and press ink go. Returns the box's right edge.
    pub(crate) fn link(
        &mut self,
        action: ControlAction,
        layer: ControlLayer,
        text: Rc<ShapedText>,
        x: f32,
        y: f32,
        h: f32,
    ) -> f32 {
        let pad = gap::GROUP / 2.0;
        let w = link_width(&text);
        let row_h = self.geometry.row_height;
        let text_layer = match layer {
            ControlLayer::Body => FULL,
            ControlLayer::Header => HEADERS,
        };
        self.text(text_layer, x + pad, y + ((h - row_h) / 2.0).max(0.0), text);
        let box_h = height::SM.min(h);
        self.control(action, layer, x, y + (h - box_h) / 2.0, w, box_h);
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
        let cols = self.columns(f, Some(file));
        let (x0, width) = cols.pane(Pane::Full);
        self.row_tint(FULL, &cols, Pane::Full, kind, y, h);
        self.cursor_tint(FULL, f, (old, new), x0, width, y, h);
        let number_kind = number_kind(kind);
        for (side, n) in [(Side::Old, old), (Side::New, new)] {
            if let (Some(n), Some(column)) = (n, cols.number_column(side)) {
                let right = cols.number_right(Pane::Full, column);
                self.number(FULL, n + 1, number_kind, right, y);
            }
        }
        self.indicator(FULL, kind, &cols, Pane::Full, y, h);
        let (side, line) = match (kind, old, new) {
            (LineKind::Removed, Some(o), _) => (Side::Old, o),
            (_, _, Some(n)) => (Side::New, n),
            (_, Some(o), None) => (Side::Old, o),
            _ => return,
        };
        let (text, rows) = self.code(f, file, side, line, &cols, Pane::Full, y, paired);
        self.code_cell(
            file,
            &cols,
            Pane::Full,
            LineCell {
                file_idx: f,
                old,
                new,
                side,
                x: x0,
                code_x: cols.code_x(Pane::Full),
                right: x0 + width,
                y,
                h,
                text: text.clone(),
            },
        );
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
        let cols = self.columns(f, Some(file));
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
            self.row_tint(layer, &cols, pane, cell.kind, y, h);
            let side = if k == 0 { Side::Old } else { Side::New };
            let lines = match side {
                Side::Old => (Some(cell.line), None),
                Side::New => (None, Some(cell.line)),
            };
            self.cursor_tint(layer, f, lines, x, w, y, h);
            let right = cols.number_right(pane, 0);
            self.number(layer, cell.line + 1, number_kind(cell.kind), right, y);
            self.indicator(layer, cell.kind, &cols, pane, y, h);
            let paired = cell.pair.is_some();
            let (text, r) = self.code(f, file, side, cell.line, &cols, pane, y, paired);
            self.code_cell(
                file,
                &cols,
                pane,
                LineCell {
                    file_idx: f,
                    old: lines.0,
                    new: lines.1,
                    side,
                    x,
                    code_x: cols.code_x(pane),
                    right: x + w,
                    y,
                    h,
                    text: text.clone(),
                },
            );
            rows = rows.max(r);
            cells[k] = Some((cell.line + 1, marker(cell.kind), text));
        }
        // The divider is the left half's last point, drawn after its
        // background (the right half's quads start at `half`).
        let line = stroke::BORDER;
        self.quad(1, cols.half - line, y, line, h, self.theme.border);
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

    /// Records a clickable control at viewport-relative `(x, y)`, its ink
    /// rounded by its height (`radius::for_height`).
    pub(crate) fn control(
        &mut self,
        action: ControlAction,
        layer: ControlLayer,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) {
        self.rounded_control(action, layer, (x, y, w, h), radius::for_height(h));
    }

    /// [`Painter::control`] with its ink rounded at `radius` (a capsule's,
    /// say).
    pub(crate) fn rounded_control(
        &mut self,
        action: ControlAction,
        layer: ControlLayer,
        (x, y, w, h): (f32, f32, f32, f32),
        radius: f32,
    ) {
        self.frame.controls.push(Control {
            action,
            bounds: self.bounds_at(x, y, w, h),
            layer,
            radius,
            run: None,
        });
    }

    /// Viewport-relative `(x, y, w, h)` in window coordinates.
    pub(crate) fn bounds_at(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        let o = self.bounds.origin;
        Bounds::new(point(o.x + px(x), o.y + px(y)), size(px(w), px(h)))
    }

    /// Queues a rounded rectangle with a `stroke::BORDER` border (when
    /// `border` is set) at viewport-relative coordinates.
    pub(crate) fn rounded(
        &mut self,
        layer: usize,
        (x, y, w, h): (f32, f32, f32, f32),
        background: Hsla,
        border: Option<Hsla>,
        radius: f32,
    ) {
        let bounds = self.bounds_at(x, y, w, h);
        let layer = self.layer(layer);
        self.frame.layers[layer].rounded.push(RoundedQuad {
            bounds,
            background,
            border,
            border_widths: Edges::all(px(stroke::BORDER)),
            radius: Corners::all(px(radius)),
        });
    }

    /// A changed row's tint in `pane` (with backgrounds on): the row's
    /// background, and the stronger gutter color behind its numbers.
    fn row_tint(
        &mut self,
        layer: usize,
        cols: &Columns,
        pane: Pane,
        kind: LineKind,
        y: f32,
        h: f32,
    ) {
        let theme = self.theme;
        let (background, gutter) = match kind {
            LineKind::Removed => (theme.removed_background, theme.removed_gutter),
            LineKind::Added => (theme.added_background, theme.added_gutter),
            LineKind::Context => return,
        };
        if !self.style.backgrounds {
            return;
        }
        let (x, w) = cols.pane(pane);
        self.quad(layer, x, y, w, h, background);
        self.quad(layer, x, y, cols.gutter_right(pane) - x, h, gutter);
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
        // Wrapped at the layout's width, which a held layout keeps.
        let wrap_width = if self.style.wrap {
            cols.with_width(self.layout_width).code_width(pane).floor()
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
        let geometry = self.geometry;
        let found = self.find.as_deref_mut().map(|find| {
            let rects = find.rects(key, file, side, line, &shaped, geometry, wrap_width);
            (rects, find.current_in(f, side, line))
        });
        if let Some((rects, current)) = found {
            for m in rects.iter() {
                let color = if current.as_ref() == Some(&m.range) {
                    self.theme.find_match_current
                } else {
                    self.theme.find_match
                };
                let w = &m.rect;
                self.quad(
                    layer,
                    x + w.x0,
                    y + w.row as f32 * row_h,
                    (w.x1 - w.x0).max(0.0),
                    row_h,
                    color,
                );
            }
        }
        self.text(layer, x, y, shaped.clone());
        let rows = shaped.shaped.visual_rows();
        (shaped, rows)
    }

    /// A single-line label in one color in the code font, cached by
    /// content.
    pub(crate) fn label(&mut self, text: &str, slot: u8, color: Hsla) -> Rc<ShapedText> {
        self.label_in(text, slot, color, FONT_CODE)
    }

    /// [`Painter::label`] in the UI font.
    pub(crate) fn ui_label(&mut self, text: &str, slot: u8, color: Hsla) -> Rc<ShapedText> {
        self.label_in(text, slot, color, FONT_UI)
    }

    fn label_in(&mut self, text: &str, slot: u8, color: Hsla, font: u8) -> Rc<ShapedText> {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = TextKey::Label {
            hash: hasher.finish(),
            color: slot,
            font,
        };
        let shaper = Shaper {
            theme: self.theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        let font = if font == FONT_UI {
            self.ui_font
        } else {
            self.font
        };
        self.cache
            .get_or_shape(key, || shaper.label_in(text, color, font))
    }

    /// Queues SVG icon `path` (`icons/<name>.svg`) in `color`, `size` px
    /// square, at viewport-relative `(x, y)`.
    pub(crate) fn icon(
        &mut self,
        path: impl Into<SharedString>,
        x: f32,
        y: f32,
        size: f32,
        color: Hsla,
    ) {
        self.turned_icon(path, (x, y, size), color, 0.0);
    }

    /// [`Painter::icon`] turned `degrees` about its centre.
    pub(crate) fn turned_icon(
        &mut self,
        path: impl Into<SharedString>,
        (x, y, size): (f32, f32, f32),
        color: Hsla,
        degrees: f32,
    ) {
        let bounds = self.bounds_at(x, y, size, size);
        self.frame.icons.push((bounds, path.into(), color, degrees));
    }

    /// The layer `layer` goes to: a row layer's [`REVEAL`] twin while the
    /// revealing body paints.
    fn layer(&self, layer: usize) -> usize {
        if self.in_reveal && layer < REVEAL {
            layer + REVEAL
        } else {
            layer
        }
    }

    /// Queues a quad at viewport-relative coordinates.
    pub(crate) fn quad(&mut self, layer: usize, x: f32, y: f32, w: f32, h: f32, color: Hsla) {
        let bounds = self.bounds_at(x, y, w, h);
        let layer = self.layer(layer);
        self.frame.layers[layer].quads.push((bounds, color));
    }

    /// Queues text with its top-left corner at viewport-relative `(x, y)`.
    pub(crate) fn text(&mut self, layer: usize, x: f32, y: f32, text: Rc<ShapedText>) {
        #[cfg(feature = "debug-inspect")]
        self.debug_text.push((x, y, text.clone()));
        let o = self.bounds.origin;
        let layer = self.layer(layer);
        self.frame.layers[layer]
            .texts
            .push((point(o.x + px(x), o.y + px(y)), text));
    }
}

fn number_kind(kind: LineKind) -> NumberKind {
    match kind {
        LineKind::Context => NumberKind::Context,
        LineKind::Added => NumberKind::Added,
        LineKind::Removed => NumberKind::Removed,
    }
}

fn marker(kind: LineKind) -> char {
    match kind {
        LineKind::Context => ' ',
        LineKind::Removed => '-',
        LineKind::Added => '+',
    }
}

/// The width of a [`Painter::link`] showing `text`.
pub(crate) fn link_width(text: &ShapedText) -> f32 {
    text.shaped.width() + gap::GROUP
}

pub(crate) fn pane_layer(pane: Pane) -> usize {
    match pane {
        Pane::Full => FULL,
        Pane::Half(0) => 1,
        Pane::Half(_) => 2,
    }
}

/// Where file `f`'s header is painted at `scroll_top`, relative to the
/// viewport: its place in the document, or pinned at the top while its body
/// scrolls under it (its body's end pushing it up).
pub(crate) fn painted_header_y(doc: &Document, f: u32, scroll_top: f64) -> f32 {
    let top = (doc.header_top(f) - scroll_top) as f32;
    if top >= 0.0 {
        return top;
    }
    let body_bottom = doc.body_top(f) + f64::from(doc.body_height(f));
    ((body_bottom - scroll_top) as f32 - doc.metrics().header_height).min(0.0)
}
