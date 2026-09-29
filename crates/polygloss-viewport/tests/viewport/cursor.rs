//! Line cursor, ranges, gutter "+", selection and copy (T3.8, design §11.6
//! "Cursor", "Commenting", "Selection").

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use polygloss_diff::Side;
use polygloss_viewport::{
    CursorPos, DiffViewport, Direction, LayoutMode, PlusDebug, ViewportEvent,
};

use crate::support::*;

/// `a.rs`: 30 lines with line 5 changed and line 20 replaced, so its rows
/// are a 2-line gap, hunk `2..9`, an 8-line gap, hunk `17..24` and a 6-line
/// gap. `b.rs`: 5 added lines.
fn two_files() -> Arc<MemProvider> {
    let old = numbered("a", 30);
    let mut new = old.clone();
    new[5] = "A 5\n".to_owned();
    new[20] = "new 20\n".to_owned();
    MemProvider::new(vec![
        Spec::modified("a.rs", &old.concat(), &new.concat()),
        Spec::added("b.rs", &numbered("b", 5).concat()),
    ])
}

fn pos(file_idx: u32, side: Side, line: u32) -> CursorPos {
    CursorPos {
        file_idx,
        side,
        line,
        range_start: None,
    }
}

fn ranged(file_idx: u32, side: Side, line: u32, range_start: u32) -> CursorPos {
    CursorPos {
        range_start: Some(range_start),
        ..pos(file_idx, side, line)
    }
}

fn cursor(view: &Entity<DiffViewport>, cx: &mut VisualTestContext) -> Option<CursorPos> {
    view.read_with(cx, |v, _| v.cursor())
}

fn move_cursor(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, dir: Direction) {
    view.update(cx, |v, cx| v.move_cursor(dir, cx));
    settle(cx);
}

fn extend(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, dir: Direction) {
    view.update(cx, |v, cx| v.extend_selection(dir, cx));
    settle(cx);
}

fn set_cursor(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, at: CursorPos) {
    view.update(cx, |v, cx| v.set_cursor(Some(at), cx));
    settle(cx);
}

/// Events other than frame stats, in order.
fn events_of(events: &Rc<RefCell<Vec<ViewportEvent>>>) -> Vec<ViewportEvent> {
    events
        .borrow()
        .iter()
        .filter(|e| !matches!(e, ViewportEvent::FrameStats(_)))
        .cloned()
        .collect()
}

fn comments(events: &Rc<RefCell<Vec<ViewportEvent>>>) -> Vec<ViewportEvent> {
    events_of(events)
        .into_iter()
        .filter(|e| matches!(e, ViewportEvent::CommentRequested { .. }))
        .collect()
}

fn comment(file_idx: u32, side: Side, start_line: u32, line: u32) -> ViewportEvent {
    ViewportEvent::CommentRequested {
        file_idx,
        side,
        start_line,
        line,
    }
}

// Unified geometry of `two_files` (3 digits: 4-column numbers, two of them,
// then the 2-column indicator): code starts at 78 px. Rows: header 0..40,
// gap 40..72, then 20 px lines: ctx 2 at 72, ctx 3 at 92, ctx 4 at 112,
// -5 at 132, +5 at 152, ctx 6 at 172.
const CODE_X: f32 = 10.0 * ADVANCE;
fn row_y(i: u32) -> f32 {
    72.0 + i as f32 * ROW_H
}

#[gpui_kit::test]
fn cursor_moves_across_rows_and_files(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, two_files(), opts, 1000., 2000.);
    let (events, _sub) = record_events(&view, cx);
    assert_eq!(cursor(&view, cx), None);
    // The first cursor goes on the first line shown: gaps are skipped.
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 2)));
    assert!(
        events_of(&events).contains(&ViewportEvent::CursorMoved {
            file_idx: 0,
            side: Side::New,
            line: 2,
        }),
        "{:?}",
        events_of(&events)
    );
    // The cursor row is tinted.
    let tint = quads_of(cx, theme.cursor_line);
    assert!(
        tint.iter().any(|&(_, y, _, h)| y == row_y(0) && h == ROW_H),
        "{tint:?}"
    );
    // Context rows are on the new side, removed rows on the old one.
    for expected in [
        pos(0, Side::New, 3),
        pos(0, Side::New, 4),
        pos(0, Side::Old, 5),
        pos(0, Side::New, 5),
        pos(0, Side::New, 6),
        pos(0, Side::New, 7),
        pos(0, Side::New, 8),
        // Over the 8-line gap to the next hunk.
        pos(0, Side::New, 17),
    ] {
        move_cursor(&view, cx, Direction::Down);
        assert_eq!(cursor(&view, cx), Some(expected));
    }
    for _ in 0..7 {
        move_cursor(&view, cx, Direction::Down);
    }
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 23)));
    // Past the trailing gap into the next file, and back.
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(1, Side::New, 0)));
    move_cursor(&view, cx, Direction::Up);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 23)));
    // The ends stop the cursor.
    set_cursor(&view, cx, pos(1, Side::New, 4));
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(1, Side::New, 4)));

    // Split: a changed pair is one row; the cursor keeps its side.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    set_cursor(&view, cx, pos(0, Side::Old, 4));
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::Old, 5)));
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::Old, 6)));
}

#[gpui_kit::test]
fn cursor_scrolls_into_view_and_loads_far_files(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // A 400-line file puts the next one far outside the materialized window.
    let provider = MemProvider::new(vec![
        Spec::added("big.rs", &numbered("big", 400).concat()),
        Spec::added("next.rs", &numbered("next", 5).concat()),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    set_cursor(&view, cx, pos(0, Side::New, 0));
    // Keep moving down: the viewport follows the cursor.
    for _ in 0..30 {
        move_cursor(&view, cx, Direction::Down);
    }
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 30)));
    let top = view.read_with(cx, |v, _| v.document().scroll_top());
    // Line 30's row is at 40 + 600 px; it must be above the bottom edge.
    assert!(top > 0.0 && 40.0 + 620.0 <= top + 400.0, "scroll_top {top}");
    // `n` to a file that was never loaded: the cursor lands once it is.
    view.update(cx, |v, cx| v.next_file(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(1, Side::New, 0)));
    assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), 1);
    view.update(cx, |v, cx| v.prev_file(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 0)));
    assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), 0);
    // `k` from the next file's first line goes to the far end of this one.
    set_cursor(&view, cx, pos(1, Side::New, 0));
    move_cursor(&view, cx, Direction::Up);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 399)));
}

#[gpui_kit::test]
fn shift_arrow_extends_range_on_one_side(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    let (events, _sub) = record_events(&view, cx);
    // On the old side, the added row between is skipped.
    set_cursor(&view, cx, pos(0, Side::Old, 4));
    extend(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::Old, 5, 4)));
    extend(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::Old, 6, 4)));
    extend(&view, cx, Direction::Up);
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::Old, 5, 4)));
    // `c` comments on the range, first line first.
    view.update(cx, |v, cx| v.request_comment(cx));
    assert_eq!(comments(&events), [comment(0, Side::Old, 4, 5)]);
    // Upward ranges work too, and a range never crosses a gap.
    set_cursor(&view, cx, pos(0, Side::New, 3));
    extend(&view, cx, Direction::Up);
    extend(&view, cx, Direction::Up);
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::New, 2, 3)));
    // A plain move drops the range.
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 3)));
    // Range lines are tinted with the selection color.
    let theme = view.read_with(cx, |v, _| v.options().theme.clone());
    set_cursor(&view, cx, pos(0, Side::New, 2));
    extend(&view, cx, Direction::Down);
    let tint = quads_of(cx, theme.selection);
    for i in 0..2 {
        assert!(
            tint.iter().any(|&(_, y, _, h)| y == row_y(i) && h == ROW_H),
            "row {i}: {tint:?}"
        );
    }
    // In split, the new side of a changed pair.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    set_cursor(&view, cx, pos(0, Side::New, 4));
    extend(&view, cx, Direction::Down);
    extend(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::New, 6, 4)));
}

#[gpui_kit::test]
fn bracket_keys_jump_between_changes(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    let next = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext| {
        view.update(cx, |v, cx| v.next_change(cx));
        settle(cx);
        cursor(view, cx)
    };
    let prev = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext| {
        view.update(cx, |v, cx| v.prev_change(cx));
        settle(cx);
        cursor(view, cx)
    };
    assert_eq!(next(&view, cx), Some(pos(0, Side::Old, 5)));
    assert_eq!(next(&view, cx), Some(pos(0, Side::Old, 20)));
    // The added file is one change.
    assert_eq!(next(&view, cx), Some(pos(1, Side::New, 0)));
    assert_eq!(next(&view, cx), Some(pos(1, Side::New, 0)));
    assert_eq!(prev(&view, cx), Some(pos(0, Side::Old, 20)));
    // From inside a change block, back to its start.
    set_cursor(&view, cx, pos(0, Side::New, 20));
    assert_eq!(prev(&view, cx), Some(pos(0, Side::Old, 20)));
    assert_eq!(prev(&view, cx), Some(pos(0, Side::Old, 5)));
    assert_eq!(prev(&view, cx), Some(pos(0, Side::Old, 5)));
}

#[gpui_kit::test]
fn n_p_jump_between_files(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 400.);
    view.update(cx, |v, cx| v.next_file(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(1, Side::New, 0)));
    let anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(anchor.file_idx, 1);
    // The last file: `n` stays.
    view.update(cx, |v, cx| v.next_file(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(1, Side::New, 0)));
    view.update(cx, |v, cx| v.prev_file(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 2)));
    assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), 0);
}

#[gpui_kit::test]
fn gutter_plus_on_hover_emits_comment_requested(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    let (events, _sub) = record_events(&view, cx);
    assert_eq!(debug(&view, cx).plus_button, None);
    // Hovering the line numbers of context line 3 shows the "+".
    let at = point(px(20.), px(row_y(1) + 10.));
    cx.simulate_mouse_move(at, None, Modifiers::default());
    settle(cx);
    let plus: PlusDebug = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.file_idx, plus.side, plus.line), (0, Side::New, 3));
    let (x, y, w, h) = plus.bounds;
    assert!(y >= row_y(1) && y + h <= row_y(2), "{:?}", plus.bounds);
    assert!(x + w <= CODE_X + 1.0, "{:?}", plus.bounds);
    // On a removed line it is the old side's.
    cx.simulate_mouse_move(
        point(px(20.), px(row_y(3) + 10.)),
        None,
        Modifiers::default(),
    );
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.side, plus.line), (Side::Old, 5));
    // Not over code.
    cx.simulate_mouse_move(
        point(px(300.), px(row_y(1) + 10.)),
        None,
        Modifiers::default(),
    );
    settle(cx);
    assert_eq!(debug(&view, cx).plus_button, None);

    // Clicking it asks for a comment on that line and puts the cursor there.
    cx.simulate_mouse_move(at, None, Modifiers::default());
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    let (x, y, w, h) = plus.bounds;
    click_at(cx, x + w / 2.0, y + h / 2.0);
    assert_eq!(comments(&events), [comment(0, Side::New, 3, 3)]);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 3)));

    // In split, the left numbers are the old side's.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    cx.simulate_mouse_move(
        point(px(10.), px(row_y(1) + 10.)),
        None,
        Modifiers::default(),
    );
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.side, plus.line), (Side::Old, 3));
    cx.simulate_mouse_move(
        point(px(510.), px(row_y(1) + 10.)),
        None,
        Modifiers::default(),
    );
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.side, plus.line), (Side::New, 3));
}

#[gpui_kit::test]
fn drag_line_numbers_selects_range(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    let (events, _sub) = record_events(&view, cx);
    let from = point(px(20.), px(row_y(0) + 10.));
    let to = point(px(20.), px(row_y(2) + 10.));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    // While dragging, the range follows the pointer.
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::New, 4, 2)));
    assert!(comments(&events).is_empty());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert_eq!(comments(&events), [comment(0, Side::New, 2, 4)]);
    assert_eq!(cursor(&view, cx), Some(ranged(0, Side::New, 4, 2)));

    // Upward, over a removed row (old side only): the range stays on the new
    // side and skips it.
    events.borrow_mut().clear();
    let from = point(px(20.), px(row_y(4) + 10.));
    let to = point(px(20.), px(row_y(2) + 10.));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        point(px(20.), px(row_y(3) + 10.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert_eq!(comments(&events), [comment(0, Side::New, 4, 5)]);
}

#[gpui_kit::test]
fn copy_excludes_gutters_and_markers(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    // Drag through the code from "a 3" (after "a ") to "A 5" (after "A").
    let from = point(px(CODE_X + 2.0 * ADVANCE + 1.0), px(row_y(1) + 10.));
    let to = point(px(CODE_X + ADVANCE + 1.0), px(row_y(4) + 10.));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    // Source text only: no line numbers, no `+`/`-`, and the removed line
    // (the other side) is not part of it.
    let text = view.read_with(cx, |v, _| v.selected_text());
    assert_eq!(text.as_deref(), Some("3\na 4\nA"));
    view.update(cx, |v, cx| v.copy(cx));
    let clip = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("3\na 4\nA"));
    // The selection is painted (quads snap to device pixels).
    let theme = view.read_with(cx, |v, _| v.options().theme.clone());
    let sel = quads_of(cx, theme.selection);
    assert!(
        sel.iter()
            .any(|&(x, y, _, _)| (x - (CODE_X + 2.0 * ADVANCE)).abs() <= 0.5 && y == row_y(1)),
        "{sel:?}"
    );

    // A click drops the text selection; a line range copies whole lines.
    click_at(cx, CODE_X + 5.0, row_y(0) + 10.);
    assert_eq!(view.read_with(cx, |v, _| v.selected_text()), None);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 2)));
    extend(&view, cx, Direction::Down);
    view.update(cx, |v, cx| v.copy(cx));
    let clip = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("a 2\na 3\n"));

    // Split: a selection stays in the side it started in.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    let left_code = 6.0 * ADVANCE;
    let from = point(px(left_code + 1.0), px(row_y(2) + 10.));
    let to = point(px(700.), px(row_y(3) + 10.));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    let text = view.read_with(cx, |v, _| v.selected_text());
    assert_eq!(text.as_deref(), Some("a 4\na 5"));
}

#[gpui_kit::test]
fn expand_context_reveals_20_lines_toward_the_cursor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Changes at lines 10 and 80 of 100: a 63-line gap between the hunks.
    let old = numbered("x", 100);
    let mut new = old.clone();
    new[10] = "X 10\n".to_owned();
    new[80] = "X 80\n".to_owned();
    let provider = MemProvider::new(vec![Spec::modified("x.rs", &old.concat(), &new.concat())]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 2000.);
    // Below the gap (nearer to it than to the trailing gap): its last 20
    // lines.
    set_cursor(&view, cx, pos(0, Side::New, 78));
    view.update(cx, |v, cx| v.expand_context(cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        [(0, vec![[57, 77]])]
    );
    // Above it: its first 20.
    set_cursor(&view, cx, pos(0, Side::New, 13));
    view.update(cx, |v, cx| v.expand_context(cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        [(0, vec![[14, 34], [57, 77]])]
    );
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 13)));
    // The whole file.
    view.update(cx, |v, cx| v.expand_cursor_file(cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        [(0, vec![[0, 100]])]
    );
}
