//! `ViewportDebug` (feature `debug-inspect`): what the last frame painted, as
//! text, for tests and the perf harness. Rows are recorded as references to
//! their shaped text while painting and formatted only when asked, so the
//! feature costs little per frame.

use polygloss_diff::rows::Layout;

use crate::document::ScrollAnchor;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::{DebugContent, DebugRow};

/// A snapshot of the last frame.
///
/// `visible_rows` has one entry per painted row, top to bottom:
///
/// | Row          | Text                                                     |
/// | ------------ | -------------------------------------------------------- |
/// | File header  | `== <path>` (`== <old> → <new>` for a rename)            |
/// | Unified line | `format!("{:>5} {:>5} {} {}", old, new, marker, text)`   |
/// | Split line   | `"<cell> │ <cell>"`, a cell `format!("{:>5} {} {}", n, marker, text)` (blank for an empty side) |
/// | Other        | the label shown (`⋯ 3 unchanged lines`, `Binary file`, `Loading…`) or `[block <id>]` |
///
/// Numbers are 1-based; markers are `-`, `+` or a space; text is as displayed
/// (tabs expanded, cut with `…`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewportDebug {
    pub visible_rows: Vec<String>,
    pub anchor: ScrollAnchor,
    pub layout: Layout,
    /// Lookups served by the shaped-line cache, since the viewport was created.
    pub shaped_cache_hits: u64,
    /// Lines shaped (cache misses), since the viewport was created.
    pub shaped_cache_misses: u64,
    /// `(top, height)` of every entry of `visible_rows`, relative to the
    /// viewport top.
    pub row_bounds: Vec<(f32, f32)>,
    /// Code rows painted with syntax tokens.
    pub styled_rows: u32,
}

#[cfg(feature = "debug-inspect")]
pub(crate) fn format_rows(rows: &[DebugRow]) -> (Vec<String>, Vec<(f32, f32)>, u32) {
    let cell = |c: &Option<(u32, char, std::rc::Rc<crate::text_cache::ShapedText>)>| match c {
        Some((n, m, t)) => format!("{n:>5} {m} {}", t.text()),
        None => format!("{:>5} {} {}", "", ' ', ""),
    };
    let number = |n: Option<u32>| n.map_or(String::new(), |n| n.to_string());
    let text = rows
        .iter()
        .map(|r| match &r.content {
            DebugContent::Header(t) => format!("== {}", t.text()),
            DebugContent::Label(t) => t.text().to_owned(),
            DebugContent::Unified {
                old,
                new,
                marker,
                text,
            } => format!(
                "{:>5} {:>5} {} {}",
                number(*old),
                number(*new),
                marker,
                text.text()
            ),
            DebugContent::Split { left, right } => format!("{} │ {}", cell(left), cell(right)),
            DebugContent::Block(id) => format!("[block {id}]"),
        })
        .collect();
    let bounds = rows.iter().map(|r| (r.y, r.height)).collect();
    let styled = rows.iter().filter(|r| r.styled).count() as u32;
    (text, bounds, styled)
}
