//! The gutter: line numbers (one column per side in split, two in unified;
//! design §11.6 "Line numbers") and change indicators (`+`/`-` glyphs or
//! bars, design §11.6 "Styles").

use std::rc::Rc;

use polygloss_diff::rows::LineKind;

use crate::paint_rows::Painter;
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
