//! `ViewportDebug` (feature `debug-inspect`): what the last frame painted, as
//! text, for tests and the perf harness. Rows are recorded as references to
//! their shaped text while painting and formatted only when asked, so the
//! feature costs little per frame.

use polygloss_diff::rows::Layout;

use crate::controls::ControlAction;
use crate::document::ScrollAnchor;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::{DebugContent, DebugRow};

/// A snapshot of the last frame.
///
/// `visible_rows` has one entry per painted row, top to bottom:
///
/// | Row          | Text                                                     |
/// | ------------ | -------------------------------------------------------- |
/// | File header  | `== <path>` (`== <old> → <new>` for a rename; the pinned header comes first) |
/// | Unified line | `format!("{:>5} {:>5} {} {}", old, new, marker, text)`   |
/// | Split line   | `"<cell> │ <cell>"`, a cell `format!("{:>5} {} {}", n, marker, text)` (blank for an empty side) |
/// | Markers      | `\ No newline at end of file`, both sides' joined with ` │ ` in split |
/// | Section band | `▸ <label>` (closed) or `▾ <label>` (open)                |
/// | Other        | the label shown (`⋯ 3 unchanged lines`, `Binary file`, `Loading…`), `[block <id>]`, or `[block <old> │ block <new>]` for split blocks side by side |
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
    /// Every text run painted (line numbers, markers, code, labels) as
    /// `(x, y, text)`, its top-left corner relative to the viewport's, in
    /// paint order. Unlike `visible_rows`, which is built from row data, this
    /// is what the gutter and the panes actually drew, and where.
    pub painted_text: Vec<(f32, f32, String)>,
    /// The color of each entry of `painted_text` (its first run's: a code
    /// line's first token, a title's first run).
    pub painted_text_colors: Vec<gpui_kit::Hsla>,
    /// Every SVG icon painted, in paint order.
    pub icons: Vec<IconDebug>,
    /// Every painted file header, top to bottom.
    pub headers: Vec<HeaderDebug>,
    /// Every painted section band, top to bottom.
    pub bands: Vec<BandDebug>,
    /// Every painted file card, top to bottom (none in the flat layout).
    pub cards: Vec<CardDebug>,
    /// Every clickable control painted, relative to the viewport.
    pub controls: Vec<ControlDebug>,
    /// The "+" on the hovered line numbers.
    pub plus_button: Option<PlusDebug>,
    /// The open ⋯ menu.
    pub menu: Option<MenuDebug>,
    /// Where the prelude was placed, `(x, y, width, height)` relative to the
    /// viewport; `None` when it is not set or out of view.
    pub prelude: Option<(f32, f32, f32, f32)>,
    /// Every painted file's slot: `(file_idx, y)`, its card's top (where
    /// its header sits in place) relative to the viewport, as painted (a
    /// reveal displaces the slots below its body), top to bottom.
    pub slots: Vec<(u32, f32)>,
    /// The running reveal (ADR-0030 M3), as the last frame painted it.
    pub reveal: Option<RevealDebug>,
    /// Every painted file header's chevron: `(file_idx, degrees)`, 0 open
    /// (pointing down), −90 closed, between while its body moves.
    pub chevrons: Vec<(u32, f32)>,
    /// How many times the width chose another layout and every file was
    /// laid out again, since the viewport was created
    /// ([`crate::DiffViewport::hold_layout`] holds it).
    pub relayouts: u32,
}

/// A running reveal: a file's body opening or closing (ADR-0030 M3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevealDebug {
    pub file_idx: u32,
    /// The body's painted height below its header, the card's bottom
    /// padding included: the card's frame ends this far below the header.
    pub height: f32,
    /// Where the body's rows are cut (`CARD_Y` above the frame's bottom,
    /// never above the body's top), relative to the viewport.
    pub curtain: f32,
    /// The rows' opacity: 1, or a Reduced fade's.
    pub opacity: f32,
    /// A mouse down holds it ([`crate::DiffViewport::freeze_motion`]).
    pub frozen: bool,
}

/// The "+" painted on the hovered line numbers: the line it comments on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlusDebug {
    pub file_idx: u32,
    pub side: polygloss_diff::Side,
    /// 0-based.
    pub line: u32,
    /// `(x, y, width, height)` relative to the viewport.
    pub bounds: (f32, f32, f32, f32),
}

/// A painted SVG icon.
#[derive(Debug, Clone, PartialEq)]
pub struct IconDebug {
    /// The asset path (`icons/<name>.svg`).
    pub path: String,
    /// `(x, y, width, height)` relative to the viewport.
    pub bounds: (f32, f32, f32, f32),
}

/// How a run of a header title is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleStyle {
    /// A directory, the rename arrow or the cut's `…`: muted, regular weight.
    Dim,
    /// A file name: the header color, bold.
    Bold,
}

/// A painted file header.
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderDebug {
    pub file_idx: u32,
    /// Top edge relative to the viewport (negative while the next header
    /// pushes a pinned one up).
    pub y: f32,
    /// Pinned at the top while its file's body scrolls under it.
    pub sticky: bool,
    /// As painted (cut with `…` when it does not fit).
    pub title: String,
    /// `title` in runs, as shaped.
    pub title_runs: Vec<(String, TitleStyle)>,
    /// `(additions, deletions)` as shown; `None` while unknown, when both are
    /// zero, or when the header is too narrow for them.
    pub counts: Option<(u32, u32)>,
    /// Badges shown, left to right: the change's (similarity, mode, binary,
    /// symlink, submodule, generated, LFS), then the host's flags.
    pub badges: Vec<String>,
    pub viewed: bool,
    pub collapsed: bool,
}

/// A painted section band.
#[derive(Debug, Clone, PartialEq)]
pub struct BandDebug {
    /// The section's id.
    pub id: u32,
    /// Top edge relative to the viewport.
    pub y: f32,
    pub open: bool,
    pub label: String,
    /// `(additions, deletions)` as shown; `None` before any file is counted
    /// or when the band is too narrow for them.
    pub counts: Option<(u64, u64)>,
    /// The review pills shown, left to right ("3 open threads", "agent").
    pub badges: Vec<String>,
    /// The changed-since-viewed dot is shown.
    pub dot: bool,
    /// Its links, left to right: "Show" or "Hide", then "Mark all viewed" or
    /// "Mark all unviewed".
    pub links: Vec<String>,
}

/// A painted file card: its whole outer bounds, borders included, as
/// [`crate::DiffViewport::card_bounds`] gives them in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardDebug {
    pub file_idx: u32,
    /// `(x, y, width, height)` relative to the viewport.
    pub bounds: (f32, f32, f32, f32),
}

/// A clickable control of the last frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlDebug {
    pub action: ControlAction,
    /// `(x, y, width, height)` relative to the viewport.
    pub bounds: (f32, f32, f32, f32),
}

/// The open ⋯ menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuDebug {
    pub file_idx: u32,
    /// Item labels in order, with whether each is enabled.
    pub items: Vec<(String, bool)>,
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
            DebugContent::Labels(ts) => ts.iter().map(|t| t.text()).collect::<Vec<_>>().join(" │ "),
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
            DebugContent::BlockPair(old, new) => format!("[block {old} │ block {new}]"),
            DebugContent::Band(text) => text.clone(),
        })
        .collect();
    let bounds = rows.iter().map(|r| (r.y, r.height)).collect();
    let styled = rows.iter().filter(|r| r.styled).count() as u32;
    (text, bounds, styled)
}
