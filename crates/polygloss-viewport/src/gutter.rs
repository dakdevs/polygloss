//! The gutter (ADR-0031 C1, [`Columns`]): line numbers (one column per side
//! in split, two in unified, one for a one-sided file; design §11.6 "Line
//! numbers"), tinted on changed rows, change indicators (bars at the pane's
//! left edge or `+`/`-` glyphs, design §11.6 "Styles") and the "+" shown on
//! the hovered line numbers, over their column's right edge (design §11.6
//! "Commenting": pressing there asks for a comment, dragging selects a
//! range, see [`crate::selection`]).

use std::rc::Rc;

use polygloss_diff::rows::LineKind;

use crate::header::SLOT_ON_ACCENT;
use crate::layout::{Columns, Pane};
use crate::materialize::MaterializedFile;
use crate::paint_rows::{HEADERS, LineCell, Painter};
use crate::selection::PlusHit;
use crate::space::{card, height, radius, stroke};
use crate::style::Indicators;
use crate::text_cache::{NumberKind, ShapedText, Shaper, TextKey};

impl Painter<'_> {
    /// Queues 1-based line number `n` of a row of `kind` (changed rows
    /// tint it), right-aligned at `right`.
    pub(crate) fn number(&mut self, layer: usize, n: u32, kind: NumberKind, right: f32, y: f32) {
        let text = self.number_text(n, kind);
        let x = right - text.shaped.width();
        self.text(layer, x, y, text);
    }

    fn number_text(&mut self, n: u32, kind: NumberKind) -> Rc<ShapedText> {
        let shaper = Shaper {
            theme: self.theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        let color = match kind {
            NumberKind::Context => self.theme.line_number,
            NumberKind::Added => self.theme.added_line_number,
            NumberKind::Removed => self.theme.removed_line_number,
        };
        self.cache.get_or_shape(TextKey::Number { n, kind }, || {
            shaper.label(&n.to_string(), color)
        })
    }

    /// Queues the change marker of a row of `kind` in `pane`: a bar at the
    /// pane's left edge, or a glyph centered in its indicator cell (two
    /// advances, after the gutter); nothing for context rows.
    pub(crate) fn indicator(
        &mut self,
        layer: usize,
        kind: LineKind,
        cols: &Columns,
        pane: Pane,
        y: f32,
        h: f32,
    ) {
        let (glyph, color, slot) = match kind {
            LineKind::Context => return,
            LineKind::Removed => ("-", self.theme.removed_accent, 2),
            LineKind::Added => ("+", self.theme.added_accent, 3),
        };
        match self.style.indicators {
            Indicators::PlusMinus => {
                let text = self.label(glyph, slot, color);
                let x = cols.indicator_x(pane) + 0.5 * self.geometry.advance;
                self.text(layer, x, y, text);
            }
            Indicators::Bars => {
                let bar = self.cards.map_or(stroke::CHANGE_BAR, |c| c.bar);
                self.quad(layer, cols.pane(pane).0, y, bar, h, color);
            }
            Indicators::None => {}
        }
    }
}

impl Painter<'_> {
    /// Finishes a painted code cell: its selected text, the "+" when the
    /// pointer is on its numbers, and its hit-test entry.
    pub(crate) fn code_cell(
        &mut self,
        file: &MaterializedFile,
        cols: &Columns,
        pane: Pane,
        cell: LineCell,
    ) {
        self.selection_quads(file, cols, pane, &cell);
        self.plus_button(cols, pane, &cell);
        self.frame.cells.push(cell);
    }

    /// The "+", a `height::MINI` square (fixed UI geometry), centered
    /// `card::NUMBER_PAD_R` right of the right edge of `cell`'s number column
    /// (its side's): in the space after its digits, so it covers neither
    /// them nor the code (the gutter's end, for the last column). Shown when
    /// the pointer is on the gutter and no header covers the row; drawn in
    /// the header layer.
    fn plus_button(&mut self, cols: &Columns, pane: Pane, cell: &LineCell) {
        let Some((px, py)) = self.marks.pointer else {
            return;
        };
        let line = match cell.side {
            polygloss_diff::Side::Old => cell.old,
            polygloss_diff::Side::New => cell.new,
        };
        let Some(line) = line else {
            return;
        };
        if cell.side == polygloss_diff::Side::Old && !self.marks.old_side_comments {
            return;
        }
        if self.marks.text_drag
            || px < cell.x
            || px >= cell.code_x
            || py < cell.y
            || py >= cell.y + cell.h
        {
            return;
        }
        let row = self.bounds_at(cell.x, cell.y, cell.right - cell.x, cell.h);
        if self.frame.header_areas.iter().any(|a| a.intersects(&row)) {
            return;
        }
        let row_h = self.geometry.row_height;
        let size = height::MINI;
        let column = cols.number_column(cell.side).unwrap_or(0);
        let center = cols.number_right(pane, column) + card::NUMBER_PAD_R;
        let (x, y) = (center - size / 2.0, cell.y + (row_h - size) / 2.0);
        let corner = radius::for_height(size);
        self.rounded(HEADERS, (x, y, size, size), self.theme.accent, None, corner);
        let plus = self.label("+", SLOT_ON_ACCENT, self.theme.background);
        let glyph_x = x + (size - plus.shaped.width()) / 2.0;
        self.text(HEADERS, glyph_x, cell.y, plus);
        self.frame.plus = Some(PlusHit {
            file_idx: cell.file_idx,
            side: cell.side,
            line,
            bounds: (x, y, size, size),
        });
    }
}
