//! The line cursor and line ranges (design §11.6 "Cursor", §11.9): `j`/`k`
//! move it across rows and files, `⇧↑`/`⇧↓` extend a range on one side, `]`/`[`
//! jump between changes, `n`/`p` between files. It is the target of `c`
//! (a comment request), `e` (expand the nearest gap toward it) and `E`
//! (expand its file).
//!
//! The cursor is a line of one side of one file ([`CursorPos`]), not a row,
//! so it survives relayouts (split ↔ unified, revealed context, blocks).
//! A move into a file that is not laid out yet (never materialized) scrolls
//! to it and lands once its rows exist ([`Pending`], resolved in prepaint).

use gpui_kit::Context;
use polygloss_diff::Side;
use polygloss_diff::rows::{ExpandBy, LineKind, Row};

use crate::document::{BodyRow, FileLayout, FileState, RowKey};
use crate::gap::EXPAND_STEP;
use crate::paint_rows::Painter;
use crate::view::{DiffViewport, ViewportEvent};

/// Which way the cursor moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Up,
    Down,
}

/// The line cursor: line `line` (0-based) of `side` of file `file_idx`.
/// `range_start` is the other end of a range extended with `⇧↑`/`⇧↓` or by
/// dragging across line numbers (same side, `None` without a range).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorPos {
    pub file_idx: u32,
    pub side: Side,
    pub line: u32,
    pub range_start: Option<u32>,
}

impl CursorPos {
    /// The lines the cursor covers, first to last.
    pub fn lines(&self) -> (u32, u32) {
        let start = self.range_start.unwrap_or(self.line);
        (start.min(self.line), start.max(self.line))
    }
}

/// Which row of a file a pending move lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    First,
    Last,
    FirstChange,
    LastChange,
}

/// A move waiting for file `file_idx` to be laid out. `walk` continues into
/// the next files when that file has no such row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pending {
    file_idx: u32,
    pick: Pick,
    walk: Option<Direction>,
}

/// The cursor's state in the viewport.
#[derive(Debug, Default)]
pub(crate) struct Cursor {
    pub pos: Option<CursorPos>,
    pub pending: Option<Pending>,
}

/// A row of a file's layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct At {
    f: u32,
    r: usize,
}

/// What a scan found.
enum Found {
    Row(At),
    /// A file that is not laid out yet: the scan cannot see its rows.
    Pending(u32),
}

/// How far a move scrolls to show the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reveal {
    /// Just enough, keeping two rows of margin (`j`/`k`).
    Minimal,
    /// When out of view, to a third of the viewport (jumps).
    Jump,
}

/// The lines a row shows: `(old, new)`.
fn row_lines(row: BodyRow) -> Option<(Option<u32>, Option<u32>)> {
    match row {
        BodyRow::Line { old, new, .. } => Some((old, new)),
        _ => None,
    }
}

fn line_on(lines: (Option<u32>, Option<u32>), side: Side) -> Option<u32> {
    match side {
        Side::Old => lines.0,
        Side::New => lines.1,
    }
}

impl DiffViewport {
    /// The cursor, if there is one.
    pub fn cursor(&self) -> Option<CursorPos> {
        self.cursor.pos
    }

    /// Puts the cursor at `pos` (or removes it), scrolling it into view.
    /// Hosts use it to jump to a thread or a find match.
    pub fn set_cursor(&mut self, pos: Option<CursorPos>, cx: &mut Context<Self>) {
        self.cursor.pending = None;
        self.selection = None;
        self.put_cursor(pos, cx);
        if pos.is_some() {
            self.reveal_cursor(Reveal::Jump, cx);
        }
    }

    /// `j`/`k`: the cursor to the next or previous code row, across files
    /// (gaps, blocks and markers are skipped). Without a cursor, it goes on
    /// the first line shown. Drops a range and a text selection.
    pub fn move_cursor(&mut self, dir: Direction, cx: &mut Context<Self>) {
        self.selection = None;
        let Some(pos) = self.cursor.pos else {
            self.place_at_top(cx);
            return;
        };
        let found = match self.cursor_at() {
            Some(at) => self.scan(at.f, Some(at.r), dir, true, Self::is_line),
            None => self.scan(pos.file_idx, None, dir, true, Self::is_line),
        };
        let pick = match dir {
            Direction::Down => Pick::First,
            Direction::Up => Pick::Last,
        };
        self.land(found, pos.side, pick, Some(dir), Reveal::Minimal, cx);
    }

    /// `⇧↓`/`⇧↑`: extends the cursor's range to the next or previous line of
    /// its side. A range never crosses hidden context or a file boundary.
    pub fn extend_selection(&mut self, dir: Direction, cx: &mut Context<Self>) {
        self.selection = None;
        let (Some(pos), Some(at)) = (self.cursor.pos, self.cursor_at()) else {
            self.move_cursor(dir, cx);
            return;
        };
        let Some(layout) = self.doc.file_layout(at.f) else {
            return;
        };
        let rows = layout.rows();
        let mut r = at.r;
        let next = loop {
            r = match dir {
                Direction::Down if r + 1 < rows.len() => r + 1,
                Direction::Up if r > 0 => r - 1,
                _ => break None,
            };
            match rows[r] {
                BodyRow::Gap { .. } | BodyRow::Placeholder => break None,
                row => {
                    if let Some(line) = row_lines(row).and_then(|l| line_on(l, pos.side)) {
                        break Some(line);
                    }
                }
            }
        };
        let Some(line) = next else {
            return;
        };
        let start = pos.range_start.unwrap_or(pos.line);
        self.put_cursor(
            Some(CursorPos {
                line,
                range_start: (start != line).then_some(start),
                ..pos
            }),
            cx,
        );
        self.reveal_cursor(Reveal::Minimal, cx);
    }

    /// `]`: the cursor to the first line of the next change (a run of
    /// removed and added rows), across files.
    pub fn next_change(&mut self, cx: &mut Context<Self>) {
        self.jump_change(Direction::Down, cx);
    }

    /// `[`: the cursor to the first line of the previous change (or of the
    /// change it is in).
    pub fn prev_change(&mut self, cx: &mut Context<Self>) {
        self.jump_change(Direction::Up, cx);
    }

    /// `n`: the next file's header to the top, the cursor on its first line.
    pub fn next_file(&mut self, cx: &mut Context<Self>) {
        self.jump_file(Direction::Down, cx);
    }

    /// `p`: the previous file's header to the top, the cursor on its first
    /// line.
    pub fn prev_file(&mut self, cx: &mut Context<Self>) {
        self.jump_file(Direction::Up, cx);
    }

    /// `c`: asks the host for a comment on the selected text's lines, else
    /// on the cursor's line or range ([`ViewportEvent::CommentRequested`]).
    /// Without a cursor, places one on the first line shown instead.
    pub fn request_comment(&mut self, cx: &mut Context<Self>) {
        if let Some(sel) = self.selection.filter(|s| !s.is_empty()) {
            let (start, end) = sel.ordered();
            cx.emit(ViewportEvent::CommentRequested {
                file_idx: sel.file_idx,
                side: sel.side,
                start_line: start.line,
                line: end.line,
            });
            return;
        }
        match self.cursor.pos {
            Some(pos) => {
                let (start_line, line) = pos.lines();
                cx.emit(ViewportEvent::CommentRequested {
                    file_idx: pos.file_idx,
                    side: pos.side,
                    start_line,
                    line,
                });
            }
            None => self.place_at_top(cx),
        }
    }

    /// `e`: reveals [`EXPAND_STEP`] hidden lines of the gap nearest the
    /// cursor (or the first line shown), on the side toward it: the last
    /// lines of a gap above, the first of a gap below.
    pub fn expand_context(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.cursor_at().or_else(|| self.top_row()) else {
            return;
        };
        let Some(layout) = self.doc.file_layout(at.f) else {
            return;
        };
        let nearest = layout
            .rows()
            .iter()
            .enumerate()
            .filter_map(|(r, row)| match *row {
                BodyRow::Gap { old_start, len, .. } => Some((r, old_start, len)),
                _ => None,
            })
            .min_by_key(|&(r, _, _)| (r.abs_diff(at.r), r < at.r));
        let Some((r, old_start, len)) = nearest else {
            return;
        };
        let by = if r < at.r {
            ExpandBy::Up(EXPAND_STEP)
        } else {
            ExpandBy::Down(EXPAND_STEP)
        };
        self.expand_run(at.f, old_start..old_start + len, by, cx);
        if self.cursor.pos.is_some() {
            self.reveal_cursor(Reveal::Minimal, cx);
        }
    }

    /// `E`: reveals every line of the cursor's file (or of the file at the
    /// top).
    pub fn expand_cursor_file(&mut self, cx: &mut Context<Self>) {
        let f = self
            .cursor
            .pos
            .map_or(self.doc.anchor().file_idx, |p| p.file_idx);
        self.expand_file(f, cx);
        if self.cursor.pos.is_some() {
            self.reveal_cursor(Reveal::Minimal, cx);
        }
    }

    /// Lands a pending move once its file is laid out (prepaint). Returns
    /// whether it scrolled.
    pub(crate) fn resolve_pending_cursor(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(p) = self.cursor.pending else {
            return false;
        };
        if p.file_idx >= self.doc.len() {
            self.cursor.pending = None;
            return false;
        }
        if !self.doc.is_collapsed(p.file_idx) && self.doc.file_layout(p.file_idx).is_none() {
            return false;
        }
        self.cursor.pending = None;
        let before = self.doc.scroll_top();
        let (dir, change) = match p.pick {
            Pick::First => (Direction::Down, false),
            Pick::Last => (Direction::Up, false),
            Pick::FirstChange => (Direction::Down, true),
            Pick::LastChange => (Direction::Up, true),
        };
        let prefer = self.cursor.pos.map_or(Side::New, |c| c.side);
        let found = self.scan(p.file_idx, None, dir, p.walk.is_some(), |v, f, row| {
            if change {
                v.is_change_start(f, row)
            } else {
                v.is_line(f, row)
            }
        });
        match (found, p.walk) {
            (None, None) => self.put_cursor(None, cx),
            (found, _) => self.land(found, prefer, p.pick, p.walk, Reveal::Jump, cx),
        }
        self.doc.scroll_top() != before
    }

    fn jump_change(&mut self, dir: Direction, cx: &mut Context<Self>) {
        self.selection = None;
        let prefer = self.cursor.pos.map_or(Side::New, |c| c.side);
        let start = match self.cursor_at() {
            Some(at) => Some((at.f, Some(at.r))),
            None => self.top_row().map(|at| match dir {
                // The first row shown is a candidate going down.
                Direction::Down => (at.f, at.r.checked_sub(1)),
                Direction::Up => (at.f, Some(at.r)),
            }),
        };
        let Some((f, from)) = start else {
            return;
        };
        let found = self.scan(f, from, dir, true, |v, f, r| v.is_change_start(f, r));
        let pick = match dir {
            Direction::Down => Pick::FirstChange,
            Direction::Up => Pick::LastChange,
        };
        self.land(found, prefer, pick, Some(dir), Reveal::Jump, cx);
    }

    /// File `file_idx`'s header to the top, the cursor on its first line
    /// (none when the file shows no code rows, e.g. collapsed). Hosts use it
    /// to jump to a file (the next unviewed one, T3.7).
    pub fn go_to_file(&mut self, file_idx: u32, cx: &mut Context<Self>) {
        if file_idx >= self.doc.len() {
            return;
        }
        self.selection = None;
        self.jump_to_file(file_idx, cx);
    }

    fn jump_file(&mut self, dir: Direction, cx: &mut Context<Self>) {
        self.selection = None;
        let current = self
            .cursor
            .pos
            .map_or(self.doc.anchor().file_idx, |p| p.file_idx);
        let target = match dir {
            Direction::Down => current + 1,
            Direction::Up => match current.checked_sub(1) {
                Some(f) => f,
                None => return,
            },
        };
        if target >= self.doc.len() {
            return;
        }
        self.jump_to_file(target, cx);
    }

    fn jump_to_file(&mut self, target: u32, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.cursor.pending = None;
        self.doc.scroll_to(target, RowKey::Header);
        self.after_scroll(cx);
        let prefer = self.cursor.pos.map_or(Side::New, |c| c.side);
        let found = self.scan(target, None, Direction::Down, false, Self::is_line);
        match found {
            None => self.put_cursor(None, cx),
            found => self.land(found, prefer, Pick::First, None, Reveal::Jump, cx),
        }
    }

    /// Applies a scan's result: the cursor on a row, or a pending move into
    /// a file that is not laid out yet (scrolled to, so it loads). Nothing
    /// found leaves the cursor where it is.
    fn land(
        &mut self,
        found: Option<Found>,
        prefer: Side,
        pick: Pick,
        walk: Option<Direction>,
        reveal: Reveal,
        cx: &mut Context<Self>,
    ) {
        match found {
            Some(Found::Row(at)) => {
                let Some(pos) = self.pos_at(at, prefer) else {
                    return;
                };
                self.put_cursor(Some(pos), cx);
                self.reveal_cursor(reveal, cx);
            }
            Some(Found::Pending(f)) => {
                self.cursor.pending = Some(Pending {
                    file_idx: f,
                    pick,
                    walk,
                });
                self.doc.scroll_to(f, RowKey::Header);
                self.after_scroll(cx);
            }
            None => {}
        }
    }

    /// Without a cursor: puts one on the first line shown.
    fn place_at_top(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.top_row() else {
            return;
        };
        let found = self.scan(
            at.f,
            at.r.checked_sub(1),
            Direction::Down,
            true,
            Self::is_line,
        );
        self.land(
            found,
            Side::New,
            Pick::First,
            Some(Direction::Down),
            Reveal::Minimal,
            cx,
        );
    }

    /// Sets the cursor and reports it.
    fn put_cursor(&mut self, pos: Option<CursorPos>, cx: &mut Context<Self>) {
        if self.cursor.pos == pos {
            return;
        }
        self.cursor.pos = pos;
        if let Some(p) = pos {
            cx.emit(ViewportEvent::CursorMoved {
                file_idx: p.file_idx,
                side: p.side,
                line: p.line,
            });
        }
        cx.notify();
    }

    /// The cursor at row `at`: its `prefer` side when the row shows it (in
    /// split), else the other; a unified row is on its new side unless it
    /// shows the old side only.
    fn pos_at(&self, at: At, prefer: Side) -> Option<CursorPos> {
        let lines = row_lines(self.row(at.f, at.r))?;
        let prefer = match self.layout {
            polygloss_diff::rows::Layout::Unified => Side::New,
            polygloss_diff::rows::Layout::Split => prefer,
        };
        let other = match prefer {
            Side::Old => Side::New,
            Side::New => Side::Old,
        };
        let (side, line) = match (line_on(lines, prefer), line_on(lines, other)) {
            (Some(l), _) => (prefer, l),
            (None, Some(l)) => (other, l),
            (None, None) => return None,
        };
        Some(CursorPos {
            file_idx: at.f,
            side,
            line,
            range_start: None,
        })
    }

    /// Whether row `r` of file `f` is a code row.
    fn is_line(&self, f: u32, r: usize) -> bool {
        row_lines(self.row(f, r)).is_some()
    }

    /// Row `r` of file `f`'s layout (a placeholder when there is none).
    fn row(&self, f: u32, r: usize) -> BodyRow {
        self.doc
            .file_layout(f)
            .and_then(|l| l.rows().get(r).copied())
            .unwrap_or(BodyRow::Placeholder)
    }

    /// The row showing the cursor (or the gap hiding its line), when its
    /// file is expanded and laid out.
    fn cursor_at(&self) -> Option<At> {
        let pos = self.cursor.pos?;
        if pos.file_idx >= self.doc.len() || self.doc.is_collapsed(pos.file_idx) {
            return None;
        }
        let layout = self.doc.file_layout(pos.file_idx)?;
        let r = layout.find(RowKey::Line {
            side: pos.side,
            line: pos.line,
        })?;
        Some(At { f: pos.file_idx, r })
    }

    /// The first body row shown below the pinned header (row 0 of the file
    /// at the top when its header is in view).
    fn top_row(&self) -> Option<At> {
        if self.doc.is_empty() {
            return None;
        }
        let header = f64::from(self.doc.metrics().header_height);
        let (f, y) = self.doc.file_at(self.doc.scroll_top() + header);
        let r = match self.doc.file_layout(f) {
            Some(layout) if y >= header && !self.doc.is_collapsed(f) => layout.row_at(y - header).0,
            _ => 0,
        };
        Some(At { f, r })
    }

    /// Scans rows for `pred` from row `from` of file `f` (exclusive; `None`
    /// starts at the file's first row going down, its last going up),
    /// continuing into the next files when `cross`. Collapsed files and
    /// files without rows are skipped; a file that is not laid out stops
    /// the scan ([`Found::Pending`]).
    fn scan(
        &self,
        f: u32,
        from: Option<usize>,
        dir: Direction,
        cross: bool,
        pred: impl Fn(&Self, u32, usize) -> bool,
    ) -> Option<Found> {
        let mut f = f;
        let mut from = from;
        loop {
            if f >= self.doc.len() {
                return None;
            }
            if !self.doc.is_collapsed(f) {
                let Some(layout) = self.doc.file_layout(f) else {
                    return Some(Found::Pending(f));
                };
                let n = layout.len();
                let hit = match dir {
                    Direction::Down => {
                        let start = from.map_or(0, |r| r + 1);
                        (start..n).find(|&r| pred(self, f, r))
                    }
                    Direction::Up => {
                        let end = from.unwrap_or(n).min(n);
                        (0..end).rev().find(|&r| pred(self, f, r))
                    }
                };
                if let Some(r) = hit {
                    return Some(Found::Row(At { f, r }));
                }
            }
            if !cross {
                return None;
            }
            f = match dir {
                Direction::Down => f + 1,
                Direction::Up => f.checked_sub(1)?,
            };
            from = None;
        }
    }

    /// Whether row `r` of file `f` shows a removed or added line.
    fn is_change(&self, f: u32, layout: &FileLayout, r: usize) -> bool {
        let BodyRow::Line { old, new, diff_row } = layout.rows()[r] else {
            return false;
        };
        if let FileState::Materialized(file) = self.doc.state(f) {
            match self
                .gaps
                .painted_rows(f, file, self.layout)
                .get(diff_row as usize)
            {
                Some(Row::Unified { kind, .. }) => return *kind != LineKind::Context,
                Some(Row::Split { left, right }) => {
                    return [left, right]
                        .into_iter()
                        .flatten()
                        .any(|c| c.kind != LineKind::Context);
                }
                _ => {}
            }
        }
        // Rows being rebuilt: a unified row with one side is a change.
        old.is_some() != new.is_some()
    }

    /// Whether row `r` of file `f` starts a change: a changed row whose
    /// previous code row (blocks and markers aside) is not one.
    fn is_change_start(&self, f: u32, r: usize) -> bool {
        let Some(layout) = self.doc.file_layout(f) else {
            return false;
        };
        if !self.is_change(f, layout, r) {
            return false;
        }
        let rows = layout.rows();
        let prev = (0..r).rev().find(|&p| {
            matches!(
                rows[p],
                BodyRow::Line { .. } | BodyRow::Gap { .. } | BodyRow::Placeholder
            )
        });
        !prev.is_some_and(|p| self.is_change(f, layout, p))
    }

    /// Scrolls the cursor's row into view below the pinned header.
    fn reveal_cursor(&mut self, reveal: Reveal, cx: &mut Context<Self>) {
        let Some(at) = self.cursor_at() else {
            return;
        };
        let Some(layout) = self.doc.file_layout(at.f) else {
            return;
        };
        let header = f64::from(self.doc.metrics().header_height);
        let row_h = f64::from(self.doc.metrics().row_height);
        let view_h = f64::from(self.doc.viewport_height());
        if view_h <= header {
            return;
        }
        let top = self.doc.file_top(at.f) + header + layout.row_top(at.r);
        let bottom = top + f64::from(layout.row_height(at.r));
        let scroll = self.doc.scroll_top();
        let (vis_top, vis_bottom) = (scroll + header, scroll + view_h);
        let dy = match reveal {
            Reveal::Minimal => {
                let margin = (2.0 * row_h).min(((view_h - header) / 2.0 - row_h).max(0.0));
                if top < vis_top + margin {
                    top - (vis_top + margin)
                } else if bottom > vis_bottom - margin {
                    bottom - (vis_bottom - margin)
                } else {
                    0.0
                }
            }
            Reveal::Jump => {
                if top < vis_top || bottom > vis_bottom {
                    top - (vis_top + (view_h - header) / 3.0).floor()
                } else {
                    0.0
                }
            }
        };
        if dy != 0.0 {
            self.doc.scroll_by(dy as f32);
            self.after_scroll(cx);
        }
    }
}

impl Painter<'_> {
    /// Tints a code cell of file `f` showing `lines` (`(old, new)`, pane
    /// `x..x + w`) when it holds the cursor's line (the active line color,
    /// with an accent bar at the pane's left edge) or a line of its range
    /// (the selection color).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cursor_tint(
        &mut self,
        layer: usize,
        f: u32,
        lines: (Option<u32>, Option<u32>),
        x: f32,
        w: f32,
        y: f32,
        h: f32,
    ) {
        let Some(c) = self.marks.cursor.filter(|c| c.file_idx == f) else {
            return;
        };
        let Some(line) = line_on(lines, c.side) else {
            return;
        };
        let (first, last) = c.lines();
        if line < first || line > last {
            return;
        }
        let color = if c.range_start.is_some() {
            self.theme.selection
        } else {
            self.theme.cursor_line
        };
        self.quad(layer, x, y, w, h, color);
        if line == c.line {
            self.quad(layer, x, y, CURSOR_BAR_WIDTH, h, self.theme.accent);
        }
    }
}

/// Width of the accent bar at the cursor line's left edge.
const CURSOR_BAR_WIDTH: f32 = 2.0;
