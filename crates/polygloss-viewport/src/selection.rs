//! Mouse selection and copy (design §11.6 "Commenting", "Selection"):
//! pressing on a line's numbers (the gutter) starts a line range that
//! follows the pointer and asks for a comment on release; pressing on code
//! starts a text selection within that side, copied with `⌘C` as source
//! text without line numbers or `+`/`-` markers (**Provisional**).
//!
//! Hit tests use the code cells the last frame painted
//! ([`crate::paint_rows::LineCell`]), so they match what is on screen.

use std::rc::Rc;

use gpui_kit::{
    App, ClipboardItem, Context, CursorStyle, DispatchPhase, Entity, Hitbox, HitboxBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window,
};
use polygloss_diff::Side;

use crate::cursor::CursorPos;
use crate::document::FileState;
use crate::layout::{Columns, Pane};
use crate::materialize::MaterializedFile;
use crate::paint_rows::{Frame, LineCell, Painter, pane_layer};
use crate::text_cache::{DisplayLine, MAX_CHARS_UNWRAPPED, MAX_CHARS_WRAPPED};
use crate::view::{DiffViewport, ViewportEvent};

/// A position in a source line: line (0-based) and byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TextPoint {
    pub line: u32,
    pub offset: u32,
}

/// Selected text of one side of one file, from `anchor` (where the drag
/// started) to `head`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextSelection {
    pub file_idx: u32,
    pub side: Side,
    pub anchor: TextPoint,
    pub head: TextPoint,
}

impl TextSelection {
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` in text order.
    pub fn ordered(&self) -> (TextPoint, TextPoint) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
}

/// A mouse drag in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drag {
    /// Across line numbers from line `anchor` of `side` (a range).
    Gutter {
        file_idx: u32,
        side: Side,
        anchor: u32,
    },
    /// Across code (the viewport's text selection).
    Text,
}

/// Where a point falls in a code cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Zone {
    Gutter,
    Code,
}

/// The "+" shown on the hovered line numbers, as painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlusHit {
    pub file_idx: u32,
    pub side: Side,
    pub line: u32,
    /// Viewport-relative `(x, y, width, height)`.
    pub bounds: (f32, f32, f32, f32),
}

/// The line of `side` a cell shows, if any.
fn cell_line(cell: &LineCell, side: Side) -> Option<u32> {
    match side {
        Side::Old => cell.old,
        Side::New => cell.new,
    }
}

impl Frame {
    /// The cell and zone at viewport-relative `(x, y)`.
    pub(crate) fn cell_at(&self, x: f32, y: f32) -> Option<(&LineCell, Zone)> {
        let cell = self
            .cells
            .iter()
            .find(|c| x >= c.x && x < c.right && y >= c.y && y < c.y + c.h)?;
        let zone = if x < cell.code_x {
            Zone::Gutter
        } else {
            Zone::Code
        };
        Some((cell, zone))
    }

    /// The painted cell of file `f` showing a line of `side` nearest to
    /// viewport-relative `y` (the one containing it, else the closest).
    fn nearest_cell(&self, f: u32, side: Side, y: f32) -> Option<&LineCell> {
        let distance = |c: &LineCell| {
            if y < c.y {
                c.y - y
            } else if y >= c.y + c.h {
                y - (c.y + c.h) + 0.5
            } else {
                0.0
            }
        };
        self.cells
            .iter()
            .filter(|c| c.file_idx == f && cell_line(c, side).is_some())
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    }
}

/// Inserts a hitbox over every code cell's gutter (a pointing hand: it
/// takes clicks) and code (an I-beam). Call before the controls' hitboxes.
pub(crate) fn insert_hitboxes(frame: &Frame, window: &mut Window) -> Rc<[(Hitbox, CursorStyle)]> {
    let o = frame.origin;
    let mut hitboxes = Vec::with_capacity(frame.cells.len() * 2);
    for c in &frame.cells {
        let rect = |x0: f32, x1: f32| {
            gpui_kit::Bounds::new(
                gpui_kit::point(o.x + gpui_kit::px(x0), o.y + gpui_kit::px(c.y)),
                gpui_kit::size(gpui_kit::px((x1 - x0).max(0.0)), gpui_kit::px(c.h)),
            )
        };
        let gutter = window.insert_hitbox(rect(c.x, c.code_x), HitboxBehavior::Normal);
        hitboxes.push((gutter, CursorStyle::PointingHand));
        let code = window.insert_hitbox(rect(c.code_x, c.right), HitboxBehavior::Normal);
        hitboxes.push((code, CursorStyle::IBeam));
    }
    hitboxes.into()
}

/// Sets the cells' pointer cursors and wires presses, drags and releases.
/// `viewport` is the viewport's own hitbox (under headers and popups).
pub(crate) fn wire(
    view: &Entity<DiffViewport>,
    viewport: Hitbox,
    cells: Rc<[(Hitbox, CursorStyle)]>,
    window: &mut Window,
) {
    for (hitbox, style) in cells.iter() {
        window.set_cursor_style(*style, hitbox);
    }
    let (down_view, down_hitbox) = (view.clone(), viewport.clone());
    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble
            || e.button != MouseButton::Left
            || !down_hitbox.is_hovered(window)
        {
            return;
        }
        if down_view.update(cx, |v, cx| v.mouse_down(e.position, cx)) {
            cx.stop_propagation();
        }
    });
    let move_view = view.clone();
    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx: &mut App| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        let over = viewport.is_hovered(window);
        let dragging = e.pressed_button == Some(MouseButton::Left);
        move_view.update(cx, |v, cx| v.mouse_moved(e.position, over, dragging, cx));
    });
    let up_view = view.clone();
    window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
            up_view.update(cx, |v, cx| v.mouse_up(cx));
        }
    });
}

impl DiffViewport {
    /// The selected text as source text (no line numbers, no markers), when
    /// some is selected.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let FileState::Materialized(file) = self.doc.state(sel.file_idx) else {
            return None;
        };
        let (start, end) = sel.ordered();
        let mut out = Vec::new();
        for line in start.line..=end.line {
            let bytes = file.line(sel.side, line);
            let from = if line == start.line {
                (start.offset as usize).min(bytes.len())
            } else {
                0
            };
            let to = if line == end.line {
                (end.offset as usize).min(bytes.len())
            } else {
                bytes.len()
            };
            out.extend_from_slice(&bytes[from.min(to)..to]);
            if line != end.line {
                out.push(b'\n');
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    /// `⌘C`: copies the selected text, else the cursor's lines (whole lines,
    /// each with its newline), as source text.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        let text = self.selected_text().or_else(|| {
            let pos = self.cursor.pos?;
            let FileState::Materialized(file) = self.doc.state(pos.file_idx) else {
                return None;
            };
            let (first, last) = pos.lines();
            let mut out = Vec::new();
            for line in first..=last {
                out.extend_from_slice(file.line(pos.side, line));
                out.push(b'\n');
            }
            Some(String::from_utf8_lossy(&out).into_owned())
        });
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// A left press at window position `p`: on line numbers, the cursor goes
    /// to that line and a range drag starts; on code, a text selection
    /// starts there. Returns whether it was on a code cell.
    fn mouse_down(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some((x, y)) = self.relative(p) else {
            return false;
        };
        let Some(frame) = &self.frame_pool else {
            return false;
        };
        let Some((cell, zone)) = frame.cell_at(x, y) else {
            return false;
        };
        let (f, side) = (cell.file_idx, cell.side);
        let Some(line) = cell_line(cell, side) else {
            return false;
        };
        let head = self.text_point(cell, side, x, y);
        self.cursor.pending = None;
        self.selection = None;
        match zone {
            Zone::Gutter => {
                self.drag = Some(Drag::Gutter {
                    file_idx: f,
                    side,
                    anchor: line,
                });
            }
            Zone::Code => {
                self.drag = Some(Drag::Text);
                self.selection = Some(TextSelection {
                    file_idx: f,
                    side,
                    anchor: head,
                    head,
                });
            }
        }
        self.set_mouse_cursor(
            CursorPos {
                file_idx: f,
                side,
                line,
                range_start: None,
            },
            cx,
        );
        cx.notify();
        true
    }

    /// The pointer moved: a drag follows it (within its file and side);
    /// otherwise the "+" follows the hovered line numbers.
    fn mouse_moved(
        &mut self,
        p: Point<Pixels>,
        over: bool,
        dragging: bool,
        cx: &mut Context<Self>,
    ) {
        self.pointer_inside = over;
        let rel = self.relative(p);
        match (self.drag, dragging) {
            (
                Some(Drag::Gutter {
                    file_idx,
                    side,
                    anchor,
                }),
                true,
            ) => {
                let Some((_, y)) = rel else { return };
                let line = self
                    .frame_pool
                    .as_ref()
                    .and_then(|f| f.nearest_cell(file_idx, side, y))
                    .and_then(|c| cell_line(c, side));
                if let Some(line) = line {
                    self.set_mouse_cursor(
                        CursorPos {
                            file_idx,
                            side,
                            line,
                            range_start: (line != anchor).then_some(anchor),
                        },
                        cx,
                    );
                }
            }
            (Some(Drag::Text), true) => {
                let (Some((x, y)), Some(sel)) = (rel, self.selection) else {
                    return;
                };
                let head = self
                    .frame_pool
                    .as_ref()
                    .and_then(|f| f.nearest_cell(sel.file_idx, sel.side, y))
                    .map(|c| self.text_point(c, sel.side, x, y));
                if let Some(head) = head
                    && head != sel.head
                {
                    self.selection = Some(TextSelection { head, ..sel });
                    cx.notify();
                }
            }
            _ => {
                // A release outside the window ends a drag without an up
                // event here.
                self.drag = None;
                let hovered = if over { self.plus_at(rel) } else { None };
                let painted = self
                    .frame_pool
                    .as_ref()
                    .and_then(|f| f.plus)
                    .map(|p| (p.file_idx, p.side, p.line));
                if hovered != painted {
                    cx.notify();
                }
            }
        }
    }

    /// A left release: a range drag asks for a comment on its lines; an
    /// empty text selection (a click) is dropped.
    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(Drag::Gutter { file_idx, side, .. }) => {
                let Some(pos) = self.cursor.pos.filter(|p| p.file_idx == file_idx) else {
                    return;
                };
                let (start_line, line) = pos.lines();
                cx.emit(ViewportEvent::CommentRequested {
                    file_idx,
                    side,
                    start_line,
                    line,
                });
                cx.notify();
            }
            Some(Drag::Text) if self.selection.is_some_and(|s| s.is_empty()) => {
                self.selection = None;
                cx.notify();
            }
            Some(Drag::Text) | None => {}
        }
    }

    /// The line whose numbers are under viewport-relative `rel`, as
    /// `(file, side, line)`.
    fn plus_at(&self, rel: Option<(f32, f32)>) -> Option<(u32, Side, u32)> {
        let (x, y) = rel?;
        let frame = self.frame_pool.as_ref()?;
        match frame.cell_at(x, y)? {
            (cell, Zone::Gutter) => Some((cell.file_idx, cell.side, cell_line(cell, cell.side)?)),
            _ => None,
        }
    }

    /// Window position `p` relative to the viewport's top-left corner.
    fn relative(&self, p: Point<Pixels>) -> Option<(f32, f32)> {
        let o = self.frame_pool.as_ref()?.origin;
        Some(((p.x - o.x).as_f32(), (p.y - o.y).as_f32()))
    }

    /// The source position in `cell` (of line `side`) at viewport-relative
    /// `(x, y)`: the char boundary nearest to it, the line's start left of
    /// the code and its end right of it.
    fn text_point(&self, cell: &LineCell, side: Side, x: f32, y: f32) -> TextPoint {
        let line = cell_line(cell, side).unwrap_or(0);
        let row_h = self.geometry.row_height;
        let visual_row = ((y - cell.y) / row_h).floor().max(0.0) as u32;
        let display = if x < cell.code_x {
            0
        } else if x >= cell.right {
            usize::MAX
        } else {
            cell.text
                .shaped
                .closest_index(x - cell.code_x, visual_row, row_h)
        };
        let offset = match self.doc.state(cell.file_idx) {
            FileState::Materialized(file) => {
                let max = if self.opts.style.wrap {
                    MAX_CHARS_WRAPPED
                } else {
                    MAX_CHARS_UNWRAPPED
                };
                DisplayLine::new(file.line(side, line), max).unmap(display)
            }
            _ => 0,
        };
        TextPoint { line, offset }
    }

    /// Moves the cursor where the mouse put it (no scrolling: it is on
    /// screen).
    fn set_mouse_cursor(&mut self, pos: CursorPos, cx: &mut Context<Self>) {
        if self.cursor.pos == Some(pos) {
            return;
        }
        self.cursor.pos = Some(pos);
        cx.emit(ViewportEvent::CursorMoved {
            file_idx: pos.file_idx,
            side: pos.side,
            line: pos.line,
        });
        cx.notify();
    }
}

impl Painter<'_> {
    /// The selected part of `cell`'s line, behind its text; a line the
    /// selection continues past gets half a column for its newline.
    pub(crate) fn selection_quads(
        &mut self,
        file: &MaterializedFile,
        cols: &Columns,
        pane: Pane,
        cell: &LineCell,
    ) {
        let Some(sel) = self
            .marks
            .selection
            .filter(|s| !s.is_empty() && s.file_idx == cell.file_idx)
        else {
            return;
        };
        let Some(line) = cell_line(cell, sel.side) else {
            return;
        };
        let (start, end) = sel.ordered();
        if line < start.line || line > end.line {
            return;
        }
        let src = file.line(sel.side, line);
        let max = if self.style.wrap {
            MAX_CHARS_WRAPPED
        } else {
            MAX_CHARS_UNWRAPPED
        };
        let display = DisplayLine::new(src, max);
        let from = if line == start.line {
            display.map(start.offset)
        } else {
            0
        };
        let to = if line == end.line {
            display.map(end.offset)
        } else {
            display.map(src.len() as u32)
        };
        let row_h = self.geometry.row_height;
        let (x0, row0) = cell.text.shaped.position(from, row_h);
        let (mut x1, row1) = cell.text.shaped.position(to, row_h);
        if line < end.line {
            x1 += 0.5 * self.geometry.advance;
        }
        let layer = pane_layer(pane);
        let full = cols.code_width(pane);
        for row in row0..=row1 {
            let a = if row == row0 { x0 } else { 0.0 };
            let b = if row == row1 { x1 } else { full };
            if b > a {
                let y = cell.y + row as f32 * row_h;
                self.quad(
                    layer,
                    cell.code_x + a,
                    y,
                    b - a,
                    row_h,
                    self.theme.selection,
                );
            }
        }
    }
}
