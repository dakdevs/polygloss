//! Horizontal and vertical geometry: split vs unified (design §11.6, OQ-15),
//! row heights from the code font, and the columns of a row.

use polygloss_diff::rows::{Layout, LineKind, Row};
use polygloss_diff::{FileChange, Side};

use crate::card::CardStyle;
use crate::document::{BodyRow, FileLayout, Metrics};
use crate::materialize::MaterializedFile;
use crate::numbers::one_sided;
use crate::style::Indicators;
use crate::text_cache::MAX_CHARS_WRAPPED;

/// The user's layout choice. `Auto` picks split when the viewport is at least
/// [`crate::ViewportOptions::split_min_columns`] code-font columns wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayoutMode {
    #[default]
    Auto,
    Split,
    Unified,
}

/// Columns of hysteresis around the auto threshold, so a window resized
/// across it does not flip back and forth (OQ-15).
pub const AUTO_LAYOUT_HYSTERESIS_COLUMNS: f32 = 8.0;

/// Spaces per tab stop when displaying code (**Provisional**).
pub const TAB_WIDTH: usize = 4;

/// The layout `mode` resolves to for a viewport `columns` code-font columns
/// wide. In `Auto`, the first decision is split at `split_min_columns` or
/// more; after that, unified → split needs 8 columns more and split →
/// unified 8 columns fewer (`previous` is the last auto decision).
pub fn resolve_layout(
    mode: LayoutMode,
    columns: f32,
    split_min_columns: u32,
    previous: Option<Layout>,
) -> Layout {
    let min = split_min_columns as f32;
    match mode {
        LayoutMode::Split => Layout::Split,
        LayoutMode::Unified => Layout::Unified,
        LayoutMode::Auto => {
            let threshold = match previous {
                None => min,
                Some(Layout::Split) => min - AUTO_LAYOUT_HYSTERESIS_COLUMNS,
                Some(Layout::Unified) => min + AUTO_LAYOUT_HYSTERESIS_COLUMNS,
            };
            if columns >= threshold {
                Layout::Split
            } else {
                Layout::Unified
            }
        }
    }
}

/// Code-font measurements the viewport lays out with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geometry {
    pub font_size: f32,
    /// Width of one code-font column (the advance of `0`).
    pub advance: f32,
    /// One code row.
    pub row_height: f32,
}

impl Geometry {
    pub fn new(font_size: f32, advance: f32) -> Geometry {
        let advance = if advance.is_finite() && advance > 0.0 {
            advance
        } else {
            0.6 * font_size
        };
        Geometry {
            font_size,
            advance,
            row_height: row_height_for(font_size),
        }
    }

    /// Document metrics for `layout`: a header is 2.25 rows, a gap row 1.6
    /// and a placeholder 2.4 (20/45/32/48 px at 13 px); the cards' gap and
    /// padding when `cards` is set.
    pub fn metrics(
        &self,
        layout: Layout,
        load_diff_changed_lines: u32,
        cards: Option<CardStyle>,
    ) -> Metrics {
        let row = self.row_height;
        Metrics {
            layout,
            row_height: row,
            header_height: (2.25 * row).round(),
            card_gap: cards.map_or(0.0, |c| c.gap),
            card_pad_bottom: cards.map_or(0.0, |c| c.pad_bottom),
            gap_height: (1.6 * row).round(),
            placeholder_height: (2.4 * row).round(),
            load_diff_changed_lines,
            ..Metrics::default()
        }
    }
}

/// Row height for a code font size: 1.5 × the size, rounded to whole pixels
/// so rows stay on the pixel grid (20 px at 13 px).
pub(crate) fn row_height_for(font_size: f32) -> f32 {
    (font_size * 1.5).round().max(1.0)
}

/// The layout file `change` is drawn in: unified for a one-sided file
/// (added or deleted text, [`one_sided`]), whose rows are the same in both
/// layouts; `layout` for any other.
pub(crate) fn layout_for(change: &FileChange, layout: Layout) -> Layout {
    if one_sided(change).is_some() {
        Layout::Unified
    } else {
        layout
    }
}

/// The columns of a code row, relative to the viewport's left edge: a row
/// spans `x..x + width` (a card's inner width).
///
/// Unified: `[old number][new number][indicator][code]`; a one-sided file
/// has only its side's number. Split: each half is
/// `[number][indicator][code]`, the old side on the left. Numbers are right
/// aligned in a column one character wider than the file's longest line
/// number on each side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Columns {
    pub layout: Layout,
    /// The side a one-sided file shows (its one number column).
    pub one_sided: Option<Side>,
    /// The row's left edge.
    pub x: f32,
    pub width: f32,
    pub advance: f32,
    /// Where the right half starts (split); `x + width` in unified.
    pub half: f32,
    pub number_width: f32,
    pub indicator_width: f32,
}

/// Which part of a row a cell occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    /// The whole row (unified, and full-width rows in split).
    Full,
    /// A split half: 0 = left (old), 1 = right (new).
    Half(u8),
}

impl Columns {
    pub fn new(
        layout: Layout,
        x: f32,
        width: f32,
        advance: f32,
        digits: u32,
        indicators: Indicators,
    ) -> Columns {
        let half = x + match layout {
            Layout::Split => (width / 2.0).floor(),
            Layout::Unified => width,
        };
        let indicator_width = match indicators {
            Indicators::PlusMinus => 2.0 * advance,
            Indicators::Bars | Indicators::None => 0.5 * advance,
        };
        Columns {
            layout,
            one_sided: None,
            x,
            width,
            advance,
            half,
            number_width: (digits.max(1) + 1) as f32 * advance,
            indicator_width,
        }
    }

    /// The columns of file `change` drawn in `layout`: one pane with one
    /// number column for a one-sided file ([`layout_for`]).
    pub fn for_file(self, change: &FileChange) -> Columns {
        match one_sided(change) {
            Some(side) => Columns {
                layout: Layout::Unified,
                one_sided: Some(side),
                half: self.x + self.width,
                ..self
            },
            None => self,
        }
    }

    /// Left edge and width of a pane.
    pub fn pane(&self, pane: Pane) -> (f32, f32) {
        match pane {
            Pane::Full => (self.x, self.width),
            Pane::Half(0) => (self.x, self.half - self.x),
            Pane::Half(_) => (self.half, (self.x + self.width - self.half).max(0.0)),
        }
    }

    /// Number of line-number columns in a pane.
    fn numbers(&self) -> f32 {
        match (self.layout, self.one_sided) {
            (Layout::Unified, None) => 2.0,
            _ => 1.0,
        }
    }

    /// The line-number column `side`'s numbers go into (0 = the first), or
    /// `None` when this pane has none for it (the other side of a one-sided
    /// file).
    pub fn number_column(&self, side: Side) -> Option<u8> {
        match (self.layout, self.one_sided, side) {
            (_, Some(shown), side) => (shown == side).then_some(0),
            (Layout::Unified, None, Side::New) => Some(1),
            _ => Some(0),
        }
    }

    /// Right edge of line-number column `i` (0 = old in unified) in `pane`.
    pub fn number_right(&self, pane: Pane, i: u8) -> f32 {
        let (x, _) = self.pane(pane);
        x + self.number_width * f32::from(i + 1)
    }

    /// Right edge of the last line-number column in `pane` (where the
    /// number gutter ends).
    pub fn gutter_right(&self, pane: Pane) -> f32 {
        self.pane(pane).0 + self.numbers() * self.number_width
    }

    /// Left edge of the indicator column in `pane`.
    pub fn indicator_x(&self, pane: Pane) -> f32 {
        self.gutter_right(pane)
    }

    /// Left edge of the code in `pane`.
    pub fn code_x(&self, pane: Pane) -> f32 {
        self.indicator_x(pane) + self.indicator_width
    }

    /// Width available to code in `pane`, keeping one column free at the end.
    pub fn code_width(&self, pane: Pane) -> f32 {
        let (x, w) = self.pane(pane);
        (x + w - self.code_x(pane) - self.advance).max(self.advance)
    }

    /// The pane code of `side` goes into in this layout.
    pub fn code_pane(&self, side: Side) -> Pane {
        match (self.layout, side) {
            (Layout::Unified, _) => Pane::Full,
            (Layout::Split, Side::Old) => Pane::Half(0),
            (Layout::Split, Side::New) => Pane::Half(1),
        }
    }
}

/// Decimal digits of the largest 1-based line number of a side with `lines`
/// lines, at least 3 so short files line up with longer ones.
pub(crate) fn digits(lines: u32) -> u32 {
    let mut n = lines.max(1);
    let mut d = 0;
    while n > 0 {
        n /= 10;
        d += 1;
    }
    d.max(3)
}

/// Visual rows a line of `chars` display columns needs when wrapped at
/// `wrap_width` (an estimate: wrapping at word boundaries can need more; the
/// exact count replaces it once the row is shaped).
pub(crate) fn estimated_visual_rows(chars: usize, advance: f32, wrap_width: f32) -> u32 {
    if wrap_width <= 0.0 || chars == 0 {
        return 1;
    }
    let per_row = (wrap_width / advance).floor().max(1.0) as usize;
    chars.div_ceil(per_row).max(1) as u32
}

/// Display columns of a source line (tabs to the next stop), counted up to
/// `cap` so a multi-megabyte line costs `O(cap)`.
pub(crate) fn display_columns(line: &[u8], cap: usize) -> usize {
    let mut cols = 0;
    for &b in line {
        if cols >= cap {
            return cap;
        }
        match b {
            b'\t' => cols += TAB_WIDTH - cols % TAB_WIDTH,
            b'\r' => {}
            // UTF-8 continuation bytes do not start a char.
            0x80..=0xbf => {}
            _ => cols += 1,
        }
    }
    cols.min(cap)
}

/// Row heights of `layout` (built from `rows` at one visual row each) with
/// code rows wrapped at `wrap_width`: each row is estimated from its longest
/// side's display width, so a split row is as tall as its taller side.
/// Painting replaces the estimates of visible rows with measured heights.
pub(crate) fn wrapped_heights(
    layout: &FileLayout,
    rows: &[Row],
    file: &MaterializedFile,
    advance: f32,
    wrap_width: f32,
) -> Vec<f32> {
    let visual = |side: Side, line: u32| {
        let cols = display_columns(file.line(side, line), MAX_CHARS_WRAPPED);
        estimated_visual_rows(cols, advance, wrap_width)
    };
    layout
        .rows()
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let base = layout.row_height(i);
            let BodyRow::Line { diff_row, .. } = row else {
                return base;
            };
            let n = match &rows[*diff_row as usize] {
                Row::Unified {
                    old: Some(o),
                    kind: LineKind::Removed,
                    ..
                } => visual(Side::Old, *o),
                Row::Unified { new: Some(n), .. } => visual(Side::New, *n),
                Row::Unified { old: Some(o), .. } => visual(Side::Old, *o),
                Row::Split { left, right } => {
                    let l = left.map_or(1, |c| visual(Side::Old, c.line));
                    let r = right.map_or(1, |c| visual(Side::New, c.line));
                    l.max(r)
                }
                _ => 1,
            };
            base * n as f32
        })
        .collect()
}
