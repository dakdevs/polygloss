//! The gutter: line numbers (one column per side in split, two in unified;
//! design §11.6 "Line numbers"), change indicators (`+`/`-` glyphs or
//! bars, design §11.6 "Styles") and the "+" shown on the hovered line
//! numbers (design §11.6 "Commenting": pressing there asks for a comment,
//! dragging selects a range, see [`crate::selection`]).

use std::rc::Rc;

use polygloss_diff::rows::LineKind;

use crate::header::SLOT_ON_ACCENT;
use crate::layout::{Columns, Pane};
use crate::materialize::MaterializedFile;
use crate::paint_rows::{HEADERS, LineCell, Painter};
use crate::selection::PlusHit;
use crate::style::Indicators;
use crate::text_cache::{ShapedText, Shaper, TextKey};

/// Width of an indicator bar in pixels.
pub(crate) const BAR_WIDTH: f32 = 3.0;

impl Painter<'_> {
    /// Queues 1-based line number `n`, right-aligned half a column before
    /// `right`.
    pub(crate) fn number(&mut self, layer: usize, n: u32, right: f32, y: f32) {
        let text = self.number_text(n);
        let x = right - 0.5 * self.geometry.advance - text.shaped.width();
        self.text(layer, x, y, text);
    }

    fn number_text(&mut self, n: u32) -> Rc<ShapedText> {
        let shaper = Shaper {
            theme: self.theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        let color = self.theme.line_number;
        self.cache
            .get_or_shape(TextKey::Number(n), || shaper.label(&n.to_string(), color))
    }

    /// Queues the change marker for a row of `kind` in the indicator column
    /// starting at `x`: nothing for context rows.
    pub(crate) fn indicator(&mut self, layer: usize, kind: LineKind, x: f32, y: f32, h: f32) {
        let (glyph, color, slot) = match kind {
            LineKind::Context => return,
            LineKind::Removed => ("-", self.theme.removed_accent, 2),
            LineKind::Added => ("+", self.theme.added_accent, 3),
        };
        match self.style.indicators {
            Indicators::PlusMinus => {
                let text = self.label(glyph, slot, color);
                let x = x + 0.5 * self.geometry.advance;
                self.text(layer, x, y, text);
            }
            Indicators::Bars => self.quad(layer, x, y, BAR_WIDTH, h, color),
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

    /// The "+" over the indicator column of `cell` when the pointer is on
    /// its gutter and no header covers the row. Drawn in the header layer,
    /// so it covers the row's numbers and marker.
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
        let size = (row_h - 4.0).max(8.0);
        let center = cols.indicator_x(pane) + cols.indicator_width / 2.0;
        let (x, y) = (center - size / 2.0, cell.y + (row_h - size) / 2.0);
        self.rounded(HEADERS, (x, y, size, size), self.theme.accent, None, 4.0);
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
