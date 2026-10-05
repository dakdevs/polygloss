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
// then the bars' half-column indicator): code starts at 66.3 px. Rows:
// header 0..45, gap 45..77, then 20 px lines: ctx 2 at 77, ctx 3 at 97,
// ctx 4 at 117, -5 at 137, +5 at 157, ctx 6 at 177.
const CODE_X: f32 = 8.5 * ADVANCE;
fn row_y(i: u32) -> f32 {
    HEADER_H + 32.0 + i as f32 * ROW_H
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
fn go_to_file_jumps_to_any_file_with_the_cursor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 400.);
    set_cursor(&view, cx, pos(0, Side::New, 4));
    view.update(cx, |v, cx| v.go_to_file(1, cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(1, Side::New, 0)));
    assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), 1);
    // Backwards too, and past the end does nothing.
    view.update(cx, |v, cx| v.go_to_file(0, cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 2)));
    view.update(cx, |v, cx| v.go_to_file(7, cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 2)));
    // A collapsed file: its header comes up, and the cursor leaves the
    // file it was in.
    view.update(cx, |v, cx| {
        v.set_collapsed(1, true, cx);
        v.go_to_file(1, cx);
    });
    settle(cx);
    assert_eq!(cursor(&view, cx), None);
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
    // Over the right edge of the new number column (8 columns in).
    assert!(
        (x + w / 2.0 - 8.0 * ADVANCE).abs() < 0.01,
        "{:?}",
        plus.bounds
    );
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

/// T3.12, OQ-9: with old-side comments off ("Changes since last review"),
/// old-side lines show no "+" and ask for no comment (not by `c`, a click
/// or a drag); new-side lines still do.
#[gpui_kit::test]
fn old_side_comments_off_blocks_the_plus_and_requests(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    let (events, _sub) = record_events(&view, cx);
    assert!(view.read_with(cx, |v, _| v.old_side_comments()));
    view.update(cx, |v, cx| v.set_old_side_comments(false, cx));
    settle(cx);
    assert!(!view.read_with(cx, |v, _| v.old_side_comments()));

    // No "+" on the removed line 5; context line 3 (new side) keeps it.
    let removed = point(px(20.), px(row_y(3) + 10.));
    cx.simulate_mouse_move(removed, None, Modifiers::default());
    settle(cx);
    assert_eq!(debug(&view, cx).plus_button, None);
    cx.simulate_mouse_move(
        point(px(20.), px(row_y(1) + 10.)),
        None,
        Modifiers::default(),
    );
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.side, plus.line), (Side::New, 3));

    // `c` on an old-side cursor or range asks for nothing.
    set_cursor(&view, cx, pos(0, Side::Old, 5));
    view.update(cx, |v, cx| v.request_comment(cx));
    set_cursor(&view, cx, ranged(0, Side::Old, 5, 4));
    view.update(cx, |v, cx| v.request_comment(cx));
    // Nor does pressing the removed line's numbers; the cursor still goes
    // there.
    cx.simulate_mouse_move(removed, None, Modifiers::default());
    cx.simulate_mouse_down(removed, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(removed, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert!(comments(&events).is_empty(), "{:?}", comments(&events));
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::Old, 5)));

    // The new side still asks.
    set_cursor(&view, cx, pos(0, Side::New, 3));
    view.update(cx, |v, cx| v.request_comment(cx));
    assert_eq!(comments(&events), [comment(0, Side::New, 3, 3)]);

    // Back on: the removed line's "+" returns.
    view.update(cx, |v, cx| v.set_old_side_comments(true, cx));
    cx.simulate_mouse_move(removed, None, Modifiers::default());
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.side, plus.line), (Side::Old, 5));
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

/// On a card, rows start at its inner edge, 17 px in (a 16 px margin and a
/// 1 px border); without a prelude the first card's rows are as high as in
/// the flat layout. The gutter "+" and a text selection hit the same lines
/// and characters as there, 17 px to the right.
#[gpui_kit::test]
fn gutter_plus_and_selection_hit_rows_inside_the_card(cx: &mut TestAppContext) {
    const INSET: f32 = 17.0;
    let _sb = sandbox();
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open(cx, two_files(), opts, 1000., 2000.);
    let (events, _sub) = record_events(&view, cx);
    // Hovering context line 3's numbers shows its "+", in the card's gutter.
    let at = point(px(INSET + 20.), px(row_y(1) + 10.));
    cx.simulate_mouse_move(at, None, Modifiers::default());
    settle(cx);
    let plus = debug(&view, cx).plus_button.expect("a + button");
    assert_eq!((plus.file_idx, plus.side, plus.line), (0, Side::New, 3));
    let (x, y, w, h) = plus.bounds;
    assert!(y >= row_y(1) && y + h <= row_y(2), "{:?}", plus.bounds);
    assert!(
        x >= INSET && (x + w / 2.0 - (INSET + 8.0 * ADVANCE)).abs() < 0.01,
        "{:?}",
        plus.bounds
    );
    click_at(cx, x + w / 2.0, y + h / 2.0);
    assert_eq!(comments(&events), [comment(0, Side::New, 3, 3)]);

    // Drag through the code from "a 3" (after "a ") to "A 5" (after "A").
    let from = point(px(INSET + CODE_X + 2.0 * ADVANCE + 1.0), px(row_y(1) + 10.));
    let to = point(px(INSET + CODE_X + ADVANCE + 1.0), px(row_y(4) + 10.));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    let text = view.read_with(cx, |v, _| v.selected_text());
    assert_eq!(text.as_deref(), Some("3\na 4\nA"));
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
    let left_code = 4.5 * ADVANCE;
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

#[gpui_kit::test]
fn reveal_line_shows_a_line_hidden_in_a_gap(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Changes at lines 10 and 80 of 100: a 63-line gap (old 14..77) between
    // the hunks; the new side is one line longer after line 10.
    let old = numbered("x", 100);
    let mut new = old.clone();
    new[10] = "X 10\n".to_owned();
    new.insert(11, "inserted\n".to_owned());
    new[81] = "X 80\n".to_owned();
    let provider = MemProvider::new(vec![Spec::modified("x.rs", &old.concat(), &new.concat())]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 2000.);
    // New line 41 is old line 40: revealed with 3 lines around it.
    view.update(cx, |v, cx| v.reveal_line(0, Side::New, 41, cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        [(0, vec![[37, 44]])]
    );
    assert!(
        debug(&view, cx)
            .visible_rows
            .iter()
            .any(|r| r.ends_with(" x 40")),
        "{:#?}",
        debug(&view, cx).visible_rows
    );
    // Old-side lines map to themselves; lines already shown change nothing.
    view.update(cx, |v, cx| v.reveal_line(0, Side::Old, 60, cx));
    view.update(cx, |v, cx| v.reveal_line(0, Side::New, 11, cx));
    view.update(cx, |v, cx| v.reveal_line(0, Side::Old, 40, cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        [(0, vec![[37, 44], [57, 64]])]
    );
}

#[gpui_kit::test]
fn reveal_line_before_the_file_loads_applies_once_it_does(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let old = numbered("x", 100);
    let mut new = old.clone();
    new[10] = "X 10\n".to_owned();
    new[80] = "X 80\n".to_owned();
    let provider = MemProvider::new(vec![Spec::modified("x.rs", &old.concat(), &new.concat())]);
    let (view, cx) = open_idle(cx, provider, options(LayoutMode::Unified), 1000., 2000.);
    view.update(cx, |v, cx| v.reveal_line(0, Side::New, 50, cx));
    assert!(view.read_with(cx, |v, _| v.expansions()).is_empty());
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        [(0, vec![[47, 54]])]
    );
}

#[gpui_kit::test]
fn cursor_reveal_and_top_row_use_header_top(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Cards with a 72 px prelude: a.rs's header starts at 84 (72 + a 12 px
    // gap), its body (30 rows) at 129 and ends at 729, its card at 737;
    // b.rs's header starts at 749, its body (60 rows) at 794.
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 30).concat()),
        Spec::added("b.rs", &numbered("b", 60).concat()),
    ]);
    let window = cx.open_window(gpui_kit::size(px(1000.), px(400.)), move |window, cx| {
        let opts = card_options(LayoutMode::Unified);
        let mut v = DiffViewport::new(provider, opts, window, cx);
        let prelude: polygloss_viewport::RenderBlock = Rc::new(|_, _| {
            use gpui_kit::{IntoElement as _, Styled as _};
            gpui_kit::div().h(px(72.)).into_any_element()
        });
        v.set_prelude(Some(prelude), cx);
        v
    });
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    settle(cx);

    // Line 11 of a.rs right below the pinned header (its row starts at
    // 129 + 200 = 329): without a cursor, `j` puts one on it.
    view.update(cx, |v, cx| {
        v.scroll_to(
            polygloss_viewport::ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 10,
            },
            cx,
        )
    });
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.document().scroll_top()),
        329.0 - HEADER_H as f64
    );
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 10)));

    // A jump to line 16 of b.rs (its row starts at 794 + 300 = 1094) puts it
    // a third of the way below the header: ⌊45 + 355 / 3⌋ = 163 px down.
    set_cursor(&view, cx, pos(1, Side::New, 15));
    assert_eq!(
        view.read_with(cx, |v, _| v.document().scroll_top()),
        1094.0 - 163.0
    );
    let d = debug(&view, cx);
    let i = d
        .visible_rows
        .iter()
        .position(|r| *r == unified(None, Some(16), '+', "b 15"))
        .unwrap();
    assert_eq!(d.row_bounds[i].0, 163.0);
}

#[gpui_kit::test]
fn comment_button_sits_over_the_number_column(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_files(), options(LayoutMode::Unified), 1000., 2000.);
    // Centered on the right edge of the hovered side's number column:
    // unified's new column ends at 8 columns (62.4), its old one at 4.
    let center_at = |cx: &mut VisualTestContext, x: f32, y: f32| {
        cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
        settle(cx);
        let plus = debug(&view, cx).plus_button.expect("a + button");
        let (bx, by, w, h) = plus.bounds;
        assert!(by >= y - 10.0 && by + h <= y + 10.0, "{:?}", plus.bounds);
        (plus.side, bx + w / 2.0)
    };
    let (side, x) = center_at(cx, 20.0, row_y(1) + 10.0);
    assert!(side == Side::New && (x - 8.0 * ADVANCE).abs() < 0.01, "{x}");
    let (side, x) = center_at(cx, 20.0, row_y(3) + 10.0);
    assert!(side == Side::Old && (x - 4.0 * ADVANCE).abs() < 0.01, "{x}");
    // Split: each half's one number column.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    let (side, x) = center_at(cx, 10.0, row_y(1) + 10.0);
    assert!(side == Side::Old && (x - 4.0 * ADVANCE).abs() < 0.01, "{x}");
    let (side, x) = center_at(cx, 510.0, row_y(1) + 10.0);
    assert!(
        side == Side::New && (x - (500.0 + 4.0 * ADVANCE)).abs() < 0.01,
        "{x}"
    );
    // The added file's one pane, in split too: its first row.
    let d = debug(&view, cx);
    let b = d.headers.iter().find(|h| h.file_idx == 1).expect("b.rs");
    let (side, x) = center_at(cx, 10.0, b.y + HEADER_H + 10.0);
    assert!(side == Side::New && (x - 4.0 * ADVANCE).abs() < 0.01, "{x}");
}

#[gpui_kit::test]
fn selection_on_an_added_file_copies_its_text(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::added("new.rs", "alpha\nbeta\n")]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1000., 400.);
    // One pane: code at 4.5 columns (35.1) on rows 45 and 65. From before
    // "alpha" to after "beta".
    let code_x = 4.5 * ADVANCE;
    let from = point(px(code_x + 1.0), px(HEADER_H + 10.));
    let to = point(px(code_x + 4.0 * ADVANCE + 1.0), px(HEADER_H + ROW_H + 10.));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    settle(cx);
    let text = view.read_with(cx, |v, _| v.selected_text());
    assert_eq!(text.as_deref(), Some("alpha\nbeta"));
    assert_eq!(cursor(&view, cx), Some(pos(0, Side::New, 0)));
    view.update(cx, |v, cx| v.copy(cx));
    let clip = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("alpha\nbeta"));
}

/// Five added files `f<i>.txt` of three lines, shown in the order 3, 1, 2:
/// file 0 is hidden between 1 and 2, file 4 after 2.
fn permuted_with_hidden(cx: &mut TestAppContext) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    let specs = (0..5)
        .map(|i| {
            Spec::added(
                &format!("f{i}.txt"),
                &numbered(&format!("f{i}"), 3).concat(),
            )
        })
        .collect();
    let (view, cx) = open(
        cx,
        MemProvider::new(specs),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    view.update(cx, |v, cx| {
        v.set_order(vec![3, 1, 0, 2, 4], cx);
        v.set_hidden(&[0, 4], true, cx);
    });
    settle(cx);
    (view, cx)
}

#[gpui_kit::test]
fn stepwise_keys_walk_display_order_and_skip_hidden_files(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = permuted_with_hidden(cx);
    let new = |f: u32, line: u32| Some(pos(f, Side::New, line));

    // `j`: the first cursor is on the first line shown (file 3), then down
    // through 3, 1 and 2, never into 0 or 4; it stays at the end.
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), new(3, 0));
    for (f, line) in [
        (3, 1),
        (3, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (2, 0),
        (2, 1),
        (2, 2),
        (2, 2),
    ] {
        move_cursor(&view, cx, Direction::Down);
        assert_eq!(cursor(&view, cx), new(f, line));
    }
    // `k`: back up the same way, staying at the top.
    for (f, line) in [
        (2, 1),
        (2, 0),
        (1, 2),
        (1, 1),
        (1, 0),
        (3, 2),
        (3, 1),
        (3, 0),
        (3, 0),
    ] {
        move_cursor(&view, cx, Direction::Up);
        assert_eq!(cursor(&view, cx), new(f, line));
    }

    // `]` / `[`: each added file is one change.
    let step = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext, down: bool| {
        view.update(cx, |v, cx| {
            if down {
                v.next_change(cx)
            } else {
                v.prev_change(cx)
            }
        });
        settle(cx);
        cursor(view, cx)
    };
    for f in [1, 2, 2] {
        assert_eq!(step(&view, cx, true), new(f, 0));
    }
    for f in [1, 3, 3] {
        assert_eq!(step(&view, cx, false), new(f, 0));
    }

    // `n` / `p`: file by file in display order, stopping at the last and
    // the first shown file.
    set_cursor(&view, cx, pos(3, Side::New, 1));
    for f in [1, 2, 2] {
        view.update(cx, |v, cx| v.next_file(cx));
        settle(cx);
        assert_eq!(cursor(&view, cx), new(f, 0));
        assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), f);
    }
    for f in [1, 3, 3] {
        view.update(cx, |v, cx| v.prev_file(cx));
        settle(cx);
        assert_eq!(cursor(&view, cx), new(f, 0));
    }
}

/// Five added files `f<i>.txt` of three lines with 1 and 3 in a closed
/// section (id 5): shown in the order 0, 2, 4, then the band.
fn with_closed_section(cx: &mut TestAppContext) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    let specs = (0..5)
        .map(|i| {
            Spec::added(
                &format!("f{i}.txt"),
                &numbered(&format!("f{i}"), 3).concat(),
            )
        })
        .collect();
    let (view, cx) = open(
        cx,
        MemProvider::new(specs),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    view.update(cx, |v, cx| {
        v.set_sections(
            vec![polygloss_viewport::Section {
                id: 5,
                label: "2 test files".into(),
                icon: None,
                files: vec![1, 3],
                open: false,
            }],
            cx,
        )
    });
    settle(cx);
    (view, cx)
}

#[gpui_kit::test]
fn stepwise_keys_pass_over_a_closed_section(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = with_closed_section(cx);
    let new = |f: u32, line: u32| Some(pos(f, Side::New, line));
    let next_file = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext| {
        view.update(cx, |v, cx| v.next_file(cx));
        settle(cx);
        cursor(view, cx)
    };

    // `n` from file 0: 2, then 4, the last main file, where it stays.
    set_cursor(&view, cx, pos(0, Side::New, 1));
    assert_eq!(next_file(&view, cx), new(2, 0));
    assert_eq!(next_file(&view, cx), new(4, 0));
    assert_eq!(next_file(&view, cx), new(4, 0));
    // `j` at the last line stays too; `]` finds nothing below.
    set_cursor(&view, cx, pos(4, Side::New, 2));
    move_cursor(&view, cx, Direction::Down);
    assert_eq!(cursor(&view, cx), new(4, 2));
    view.update(cx, |v, cx| v.next_change(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), new(4, 2));
    assert_eq!(view.read_with(cx, |v, _| v.section_open(5)), Some(false));

    // Open, `n` enters it: 1, then 3; `p` goes back to 4.
    view.update(cx, |v, cx| v.set_section_open(5, true, cx));
    settle(cx);
    assert_eq!(next_file(&view, cx), new(1, 0));
    assert_eq!(next_file(&view, cx), new(3, 0));
    view.update(cx, |v, cx| v.prev_file(cx));
    view.update(cx, |v, cx| v.prev_file(cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), new(4, 0));
}

#[gpui_kit::test]
fn explicit_targets_open_the_section_and_emit(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = with_closed_section(cx);
    // A block on file 1 (hidden) for the block target.
    view.update(cx, |v, cx| {
        v.set_blocks(
            1,
            vec![polygloss_viewport::BlockSpec {
                id: polygloss_viewport::BlockId(9),
                anchor: polygloss_viewport::BlockAnchor::FileTop,
                render: std::rc::Rc::new(|_, _| {
                    use gpui_kit::{IntoElement as _, Styled as _};
                    gpui_kit::div().h(px(30.)).into_any_element()
                }),
            }],
            cx,
        )
    });
    settle(cx);
    type Target = Box<dyn Fn(&mut DiffViewport, &mut gpui_kit::Context<DiffViewport>)>;
    fn target(
        f: impl Fn(&mut DiffViewport, &mut gpui_kit::Context<DiffViewport>) + 'static,
    ) -> Target {
        Box::new(f)
    }
    let line = polygloss_viewport::ScrollTarget::Line {
        file_idx: 3,
        side: Side::New,
        line: 1,
    };
    let block = polygloss_viewport::ScrollTarget::Block(polygloss_viewport::BlockId(9));
    let targets: Vec<(&str, Target)> = vec![
        (
            "scroll_to(File)",
            target(|v, cx| v.scroll_to(polygloss_viewport::ScrollTarget::File(3), cx)),
        ),
        (
            "scroll_to(Line)",
            target(move |v, cx| v.scroll_to(line, cx)),
        ),
        (
            "scroll_to(Block)",
            target(move |v, cx| v.scroll_to(block, cx)),
        ),
        ("go_to_file", target(|v, cx| v.go_to_file(3, cx))),
        (
            "set_cursor",
            target(|v, cx| v.set_cursor(Some(pos(3, Side::New, 2)), cx)),
        ),
        (
            "reveal_line",
            target(|v, cx| v.reveal_line(3, Side::New, 1, cx)),
        ),
    ];
    for (name, target) in targets {
        view.update(cx, |v, cx| {
            v.set_section_open(5, false, cx);
            v.set_cursor(None, cx);
        });
        settle(cx);
        let (events, sub) = record_events(&view, cx);
        view.update(cx, |v, cx| target(v, cx));
        settle(cx);
        assert_eq!(
            view.read_with(cx, |v, _| v.section_open(5)),
            Some(true),
            "{name}"
        );
        let toggled: Vec<ViewportEvent> = events_of(&events)
            .into_iter()
            .filter(|e| matches!(e, ViewportEvent::SectionToggled { .. }))
            .collect();
        assert_eq!(
            toggled,
            [ViewportEvent::SectionToggled { id: 5, open: true }],
            "{name}"
        );
        drop(sub);
    }
    // Where the targets went (the last ones of their kind).
    assert_eq!(cursor(&view, cx), None);
    view.update(cx, |v, cx| {
        v.set_section_open(5, false, cx);
        v.go_to_file(3, cx);
    });
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(pos(3, Side::New, 0)));
    assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), 3);

    // A target in an open section or a main file emits nothing.
    let (events, _sub) = record_events(&view, cx);
    view.update(cx, |v, cx| {
        v.go_to_file(1, cx);
        v.go_to_file(2, cx);
    });
    settle(cx);
    assert!(
        events_of(&events)
            .iter()
            .all(|e| !matches!(e, ViewportEvent::SectionToggled { .. }))
    );
}
