//! The sidebar's motion (T7.10, ADR-0030 M1 and Panels): an app-owned
//! split whose frame `f` slides 0 ↔ W̄ from the window's leading edge, the
//! content laid out at W riding its divider and the main column following
//! the same `f`; commit-first, interruptible, Reduced fading inside the held
//! frame. Every test runs a compare review in a 1,200 pt window at scale 2
//! with the sidebar at W 280 (W̄ 281, its 1 pt divider included), not
//! fullscreen. Expected values are this file's own: a bisection solver for
//! SLIDE, `round(v · 2) / 2` for the window's scale, and the durations
//! written by hand (PANEL 240 in, its exit 180 out). Steps follow ADR-0030's
//! stepping protocol (`support::motion::step_to`).

use std::time::Duration;

use gpui_kit::{
    Entity, Hsla, Modifiers, MouseButton, Point, ScrollDelta, ScrollWheelEvent, TestAppContext,
    TouchPhase, VisualTestContext, point, px, size,
};
use polygloss_app::chrome::{self, Segment};
use polygloss_app::keyboard::{self, Pane};
use polygloss_app::motion::tokens::FIRST_STEP;
use polygloss_app::motion::{self, MotionPolicy};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::tree::file_tree;
use polygloss_diff::rows::Layout;

use super::{Shell, bounds, click, compare_req, draw, painted, resize_window, start};
use crate::support::motion::{advance, frame, requested_frames, step_to};
use crate::support::{Sandbox, code_change_repo};

/// W̄: the sidebar's 280 pt and its divider.
pub const WBAR: f64 = 281.0;
/// PANEL and its exit, `exit(240)` = 180.
pub const OPEN_MS: f64 = 240.0;
pub const CLOSE_MS: f64 = 180.0;

pub fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

/// `cubic-bezier(x1, y1, x2, y2)` at `t`, by bisection on x.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let at = |a: f64, b: f64, s: f64| {
        3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..200 {
        let mid = (low + high) / 2.0;
        if at(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    at(y1, y2, (low + high) / 2.0)
}

/// SLIDE, `cubic-bezier(0.25, 1, 0.5, 1)`.
pub fn slide(t: f64) -> f64 {
    bezier(0.25, 1.0, 0.5, 1.0, t)
}

/// `round(v · 2) / 2`: the window's scale is 2.
pub fn q(v: f64) -> f32 {
    ((v * 2.0).round() / 2.0) as f32
}

/// App motion under Full, kit motion settled.
pub fn full(shell: &mut Shell) {
    set_policy(shell, MotionPolicy::Full);
}

pub fn set_policy(shell: &mut Shell, policy: MotionPolicy) {
    shell
        .cx
        .update(|_, cx| motion::set_override(Some(policy), cx));
    draw(shell.cx);
}

/// A compare review in a 1,200 × 800 window, the sidebar shown at 280 pt,
/// under Full.
pub fn review(cx: &mut TestAppContext) -> (Shell<'_>, Entity<ReviewTab>) {
    let mut shell = start(cx);
    let repo = code_change_repo();
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // The repo outlives the test's reads of it: kept in the sandbox's temp
    // dir until the process ends.
    std::mem::forget(repo);
    resize_window(&mut shell, 1200., 800.);
    full(&mut shell);
    assert_eq!(frame_width(&mut shell), 281.0, "at rest, shown");
    (shell, tab)
}

/// The sidebar's frame as laid out: where the main column starts.
pub fn frame_width(shell: &mut Shell) -> f32 {
    bounds(shell.cx, "main-column").left().as_f32()
}

pub fn visible(shell: &mut Shell) -> bool {
    shell
        .cx
        .update(|_, cx| chrome::chrome(cx).read(cx).sidebar_visible())
}

/// Hides or shows the sidebar at once (Programmatic) and draws.
pub fn set_at_once(shell: &mut Shell, shown: bool) {
    shell
        .cx
        .update(|_, cx| chrome::chrome(cx).update(cx, |c, cx| c.set_sidebar_visible(shown, cx)));
    draw(shell.cx);
    assert_eq!(visible(shell), shown);
}

/// A click on `name`'s painted centre, then the commit frame.
pub fn click_now(shell: &mut Shell, name: &str) {
    let at = bounds(shell.cx, name).center();
    shell.cx.simulate_click(at, Modifiers::none());
    frame(shell.cx);
}

/// Runs the motion committed last to its end and past it, by the stepping
/// protocol (one large step after the commit frame would be clamped to the
/// first step, ADR-0030 rule 6).
pub fn finish(shell: &mut Shell) {
    step_to(shell.cx, ms(OPEN_MS + 20.0));
}

/// The frame at `since` after the commit frame, by the stepping protocol,
/// from the commit frame drawn last.
fn sample_at(shell: &mut Shell, since: f64) -> f32 {
    step_to(shell.cx, ms(since));
    frame_width(shell)
}

#[gpui_kit::test]
fn sidebar_slides_from_the_leading_edge(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    set_at_once(&mut shell, false);
    assert_eq!(frame_width(&mut shell), 0.0);

    click_now(&mut shell, "show-sidebar");
    assert!(visible(&mut shell), "the model shows it from the commit");
    assert_eq!(
        frame_width(&mut shell),
        0.0,
        "the commit frame: at its start"
    );
    assert_eq!(
        sample_at(&mut shell, 16.667),
        q(WBAR * slide(16.667 / OPEN_MS)),
        "the first step"
    );
    advance(shell.cx, ms(OPEN_MS / 2.0) - FIRST_STEP);
    assert_eq!(frame_width(&mut shell), q(WBAR * slide(0.5)), "120 ms");
    advance(shell.cx, ms(OPEN_MS / 2.0));
    assert_eq!(frame_width(&mut shell), 281.0, "settled after 240 ms");
    assert_eq!(requested_frames(shell.cx), 0, "no frame after it settled");
}

#[gpui_kit::test]
fn sidebar_close_takes_its_exit_duration(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    click_now(&mut shell, "toggle-sidebar");
    assert!(!visible(&mut shell), "hidden in the model from the commit");
    assert_eq!(frame_width(&mut shell), 281.0, "the commit frame");
    assert_eq!(
        sample_at(&mut shell, CLOSE_MS / 2.0),
        q(WBAR * (1.0 - slide(0.5))),
        "90 ms"
    );
    advance(shell.cx, ms(CLOSE_MS / 2.0));
    assert_eq!(frame_width(&mut shell), 0.0, "closed after 180 ms");
    assert!(
        painted(shell.cx, "sidebar").is_none(),
        "its content unmounted"
    );
    assert_eq!(requested_frames(shell.cx), 0);
}

/// The content's right edge (plus the divider's point), the divider's
/// right edge and the main column's x: one value, the sampled `f`.
fn assert_one_f(shell: &mut Shell, at: &str) {
    let f = frame_width(shell);
    if f == 0.0 {
        assert!(painted(shell.cx, "sidebar").is_none(), "{at}: no content");
        assert!(painted(shell.cx, "sidebar-divider").is_none(), "{at}");
        return;
    }
    let content = bounds(shell.cx, "sidebar");
    let divider = bounds(shell.cx, "sidebar-divider");
    let pane = bounds(shell.cx, "sidebar-pane");
    assert_eq!(content.size.width, px(280.), "{at}: laid out at W");
    assert_eq!(divider.size.width, px(1.), "{at}");
    assert_eq!(
        content.right() + px(1.),
        px(f),
        "{at}: the content rides it"
    );
    assert_eq!(
        divider.right(),
        px(f),
        "{at}: the divider is its last point"
    );
    assert_eq!(pane.right(), px(f), "{at}: the frame");
}

#[gpui_kit::test]
fn frame_divider_and_main_column_share_one_f(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    for (name, end) in [("toggle-sidebar", CLOSE_MS), ("show-sidebar", OPEN_MS)] {
        click_now(&mut shell, name);
        assert_one_f(&mut shell, "the commit frame");
        step_to(shell.cx, FIRST_STEP);
        assert_one_f(&mut shell, "the first step");
        advance(shell.cx, ms(end / 2.0) - FIRST_STEP);
        assert_one_f(&mut shell, "½");
        advance(shell.cx, ms(end / 2.0));
        assert_one_f(&mut shell, "settled");
    }
}

/// Where the Reviews list's Open heading is, relative to the list: it moves
/// up as the list scrolls.
fn open_heading_y(shell: &mut Shell) -> f32 {
    (bounds(shell.cx, "nav-open").top() - bounds(shell.cx, "nav").top()).as_f32()
}

/// A wheel step of `dy` pt (positive scrolls the content up) at `at`.
fn wheel(cx: &mut VisualTestContext, at: Point<gpui_kit::Pixels>, dy: f32) {
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(-dy))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw(cx);
}

#[gpui_kit::test]
fn element_state_survives_every_frame(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    // Enough open reviews for a Reviews list taller than the sidebar.
    let repo = super::commits_repo(18);
    super::open_commits(&mut shell, repo.path(), 18);
    // The active review: a diff tall enough to scroll.
    let code = code_change_repo();
    let tab = shell.open(compare_req(code.path())).unwrap();
    resize_window(&mut shell, 1200., 480.);
    full(&mut shell);
    shell
        .cx
        .update(|_, cx| chrome::chrome(cx).update(cx, |c, cx| c.set_segment(Segment::Reviews, cx)));
    draw(shell.cx);
    let rest = open_heading_y(&mut shell);
    let nav = bounds(shell.cx, "nav").center();
    wheel(shell.cx, nav, 120.);
    assert_eq!(open_heading_y(&mut shell), rest - 120.0, "scrolled 120 pt");
    let scrolled = rest - 120.0;

    // The viewport scrolled too, and a toolbar Button has the keyboard.
    let viewport = bounds(shell.cx, "viewport-pane").center();
    wheel(shell.cx, viewport, 60.);
    let scroll_top = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            t.viewport.read(cx).document().scroll_top()
        })
    };
    let diff_top = scroll_top(&mut shell);
    assert!(diff_top > 0.0, "the diff scrolled");
    let viewport_focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    shell.cx.update(|window, cx| {
        window.focus(&viewport_focus, cx);
        window.focus_prev(cx);
    });
    draw(shell.cx);
    let button = shell
        .cx
        .update(|window, cx| window.focused(cx))
        .expect("a toolbar Button has the keyboard");
    assert!(button != viewport_focus);
    let root = shell
        .main
        .read_with(shell.cx, gpui_kit::Focusable::focus_handle);
    let held = |shell: &mut Shell, at: &str| {
        assert_eq!(scroll_top(shell), diff_top, "{at}: the diff's scroll");
        let rendered = shell.cx.update(|window, cx| {
            window.focused(cx).as_ref() == Some(&button) && root.contains(&button, window)
        });
        assert!(rendered, "{at}: the Button kept the keyboard, still drawn");
    };

    // A close: the list keeps its offset on every frame, then a re-toggle at
    // ½ opens it again from there, the list still mounted.
    let toggle = bounds(shell.cx, "toggle-sidebar").center();
    shell.cx.simulate_click(toggle, Modifiers::none());
    frame(shell.cx);
    // The click gave nothing the keyboard: the Button still has it.
    held(&mut shell, "close, the commit frame");
    assert_eq!(
        open_heading_y(&mut shell),
        scrolled,
        "close, the commit frame"
    );
    step_to(shell.cx, FIRST_STEP);
    assert_eq!(
        open_heading_y(&mut shell),
        scrolled,
        "close, the first step"
    );
    held(&mut shell, "close, the first step");
    advance(shell.cx, ms(CLOSE_MS / 2.0) - FIRST_STEP);
    assert_eq!(open_heading_y(&mut shell), scrolled, "close, ½");
    held(&mut shell, "close, ½");
    let at_half = frame_width(&mut shell);
    assert_eq!(at_half, q(WBAR * (1.0 - slide(0.5))));

    // Re-toggled at ½ by the uncovered show-sidebar button: the open runs
    // from there for 240 · (its share of the close travelled).
    let reopen = OPEN_MS * slide(0.5);
    let show = bounds(shell.cx, "show-sidebar").center();
    shell.cx.simulate_click(show, Modifiers::none());
    frame(shell.cx);
    assert!(visible(&mut shell));
    assert_eq!(frame_width(&mut shell), at_half, "the open's commit frame");
    step_to(shell.cx, FIRST_STEP);
    assert_eq!(open_heading_y(&mut shell), scrolled, "open, the first step");
    held(&mut shell, "open, the first step");
    advance(shell.cx, ms(reopen / 2.0) - FIRST_STEP);
    assert_eq!(open_heading_y(&mut shell), scrolled, "open, ½");
    held(&mut shell, "open, ½");
    advance(shell.cx, ms(reopen / 2.0 + 1.0));
    assert_eq!(frame_width(&mut shell), 281.0, "settled open");
    assert_eq!(open_heading_y(&mut shell), scrolled, "still 120 at rest");

    // M2: the threads panel opens and closes; the diff's scroll and the
    // Button's keyboard hold on every frame.
    for at in ["open", "close"] {
        click_now(&mut shell, "toggle-threads-panel");
        held(&mut shell, &format!("threads {at}, the commit frame"));
        step_to(shell.cx, FIRST_STEP);
        held(&mut shell, &format!("threads {at}, the first step"));
        advance(shell.cx, ms(100.0) - FIRST_STEP);
        held(&mut shell, &format!("threads {at}, 100 ms"));
        advance(shell.cx, ms(OPEN_MS));
        held(&mut shell, &format!("threads {at}, settled"));
    }

    // The Button is the main column's: a settled close of the sidebar,
    // whose content then unmounts, leaves it the keyboard.
    click_now(&mut shell, "toggle-sidebar");
    finish(&mut shell);
    assert!(painted(shell.cx, "nav").is_none());
    held(&mut shell, "after a settled close");
}

#[gpui_kit::test]
fn a_click_pressed_mid_close_reopens_from_the_frozen_width(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    click_now(&mut shell, "toggle-sidebar");
    let frozen = sample_at(&mut shell, CLOSE_MS / 2.0);
    // 281 · (1 − SLIDE(½)) = 18.5, well above a settled close's 0 (by
    // 170 ms it is 0.015 pt, which quantizes to 0).
    assert_eq!(frozen, q(WBAR * (1.0 - slide(0.5))));
    assert_eq!(frozen, 18.5);
    assert_eq!(q(WBAR * (1.0 - slide(170.0 / CLOSE_MS))), 0.0);
    // The show-sidebar button's painted part (all of it: f < 88).
    let at = bounds(shell.cx, "show-sidebar").center();
    assert!(at.x.as_f32() > frozen);
    shell
        .cx
        .simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    for _ in 0..3 {
        advance(shell.cx, FIRST_STEP);
        assert_eq!(frame_width(&mut shell), frozen, "frozen while pressed");
        assert_eq!(requested_frames(shell.cx), 0, "no frame while frozen");
    }
    // 200 ms after the commit, past a settled close's end.
    advance(shell.cx, ms(200.0 - CLOSE_MS / 2.0) - FIRST_STEP * 3);
    assert_eq!(frame_width(&mut shell), frozen);
    shell
        .cx
        .simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    frame(shell.cx);
    assert!(visible(&mut shell), "the click reopened it");
    assert_eq!(frame_width(&mut shell), frozen, "from the frozen width");
    step_to(shell.cx, FIRST_STEP);
    let running = frame_width(&mut shell);
    assert!(
        frozen < running && running < 281.0,
        "still opening after the mouse up: {running}"
    );
}

/// The viewport's effective layout and its relayout count.
fn viewport_layout(shell: &mut Shell, tab: &Entity<ReviewTab>) -> (Layout, u32) {
    tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        (v.effective_layout(), v.debug().relayouts)
    })
}

/// A 1,400 pt window: hidden, the viewport's cards are 1,400 − 26 = 1,374
/// pt inside, 176 columns of 7.8 pt (split from 168 up); shown, 1,400 −
/// 281 − 26 = 1,093 pt, 140 columns (unified below 152).
const FLIPS: f32 = 1400.;

#[gpui_kit::test]
fn sidebar_commit_is_one_viewport_reflow(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    resize_window(&mut shell, FLIPS, 800.);
    assert_eq!(viewport_layout(&mut shell, &tab).0, Layout::Unified);
    set_at_once(&mut shell, false);
    let (start, relayouts) = viewport_layout(&mut shell, &tab);
    assert_eq!(start, Layout::Split, "hidden, the start is split");
    click_now(&mut shell, "show-sidebar");
    let once = (Layout::Unified, relayouts + 1);
    assert_eq!(
        viewport_layout(&mut shell, &tab),
        once,
        "the commit frame: one reflow, at the target"
    );
    step_to(shell.cx, FIRST_STEP);
    assert_eq!(viewport_layout(&mut shell, &tab), once, "the first step");
    advance(shell.cx, ms(OPEN_MS / 2.0) - FIRST_STEP);
    assert_eq!(viewport_layout(&mut shell, &tab), once, "½: no flip");
    advance(shell.cx, ms(OPEN_MS / 2.0));
    frame(shell.cx);
    assert_eq!(viewport_layout(&mut shell, &tab), once, "settled");
}

/// The right half's x in split: where the two-sided cards' added rows'
/// tint starts (one-sided cards tint from the card's inner edge, left of
/// it).
fn right_half_x(shell: &mut Shell, tab: &Entity<ReviewTab>) -> f32 {
    let added = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).options().theme.added_background
    });
    shell.cx.update(|window, _| {
        let scale = window.scale_factor();
        window
            .painted_quads()
            .into_iter()
            .filter(|q| q.background.as_solid() == Some(added))
            .map(|q| q.bounds.origin.x.0 / scale)
            .fold(f32::MIN, f32::max)
    })
}

#[gpui_kit::test]
fn split_halves_keep_their_place_on_the_commit_frame(cx: &mut TestAppContext) {
    use polygloss_app::keymap::actions::viewport::LayoutSplit;
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    shell.cx.dispatch_action(LayoutSplit);
    draw(shell.cx);
    assert_eq!(viewport_layout(&mut shell, &tab).0, Layout::Split);
    // A close, then an open.
    for name in ["toggle-sidebar", "show-sidebar"] {
        let before = right_half_x(&mut shell, &tab);
        assert!(before > frame_width(&mut shell) + 200.0, "a right half");
        click_now(&mut shell, name);
        assert_eq!(
            right_half_x(&mut shell, &tab),
            before,
            "{name}: the commit frame keeps the right half's x"
        );
        finish(&mut shell);
    }
}

#[gpui_kit::test]
fn sidebar_close_is_inert_and_moves_focus_at_once(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    let pane = |shell: &mut Shell| {
        shell
            .cx
            .update(|window, cx| keyboard::focused_pane(tab.read(cx), window, cx))
    };
    let tree = tab.read_with(shell.cx, |t, _| file_tree(t).cloned().expect("a tree"));
    let selected = |shell: &mut Shell| tree.read_with(shell.cx, |t, _| t.selected_file());
    click(shell.cx, "tree-row-f:0");
    assert_eq!(
        pane(&mut shell),
        Some(Pane::Tree),
        "the tree had the keyboard"
    );
    let chosen = selected(&mut shell);
    assert!(chosen.is_some());

    click_now(&mut shell, "toggle-sidebar");
    assert_eq!(
        pane(&mut shell),
        Some(Pane::Viewport),
        "the diff has the keyboard on the first frame"
    );
    let f = sample_at(&mut shell, 60.0);
    assert!(f > 40.0, "the frame is still open: {f}");
    // A click inside the shrinking frame, over a row: it selects nothing.
    let row = bounds(shell.cx, "tree-row-f:1");
    let at = point(px(f / 2.0), row.center().y);
    shell.cx.simulate_click(at, Modifiers::none());
    frame(shell.cx);
    assert_eq!(selected(&mut shell), chosen, "nothing selected");
    assert_eq!(pane(&mut shell), Some(Pane::Viewport));
    assert!(!visible(&mut shell));
}

/// Drags the divider of split `id` (its band's centre) to window x `to`.
pub fn drag_divider(shell: &mut Shell, id: &str, to: f32) {
    let band = bounds(shell.cx, &format!("{id}-drag")).center();
    shell
        .cx
        .simulate_mouse_down(band, MouseButton::Left, Modifiers::none());
    for x in [band.x.as_f32() + 6.0, to] {
        shell.cx.simulate_mouse_move(
            point(px(x), band.y),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
    }
    shell
        .cx
        .simulate_mouse_up(point(px(to), band.y), MouseButton::Left, Modifiers::none());
    draw(shell.cx);
}

fn sidebar_width(shell: &mut Shell) -> f32 {
    bounds(shell.cx, "sidebar").size.width.as_f32()
}

#[gpui_kit::test]
fn panel_split_keeps_todays_resize_behavior(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    set_policy(&mut shell, MotionPolicy::Off);
    // The sidebar: its divider's band takes the drag, with the col-resize
    // cursor; its width is the pointer's x, within 220 and `sidebar_max`
    // (1,200 − 320 − 1, capped at 480).
    let band = bounds(shell.cx, "sidebar-drag");
    assert_eq!(band.size.width, px(9.), "the 9 pt band");
    assert_eq!(band.center().x, px(280.5), "centred on the divider");
    for (to, width) in [(320., 320.), (100., 220.), (1_100., 480.)] {
        drag_divider(&mut shell, "sidebar", to);
        assert_eq!(sidebar_width(&mut shell), width, "dragged to {to}");
        assert_eq!(
            frame_width(&mut shell),
            width + 1.0,
            "the main column after its divider"
        );
    }
    drag_divider(&mut shell, "sidebar", 320.);
    resize_window(&mut shell, 1_000., 800.);
    assert_eq!(sidebar_width(&mut shell), 320., "a window resize keeps it");
    // The main column's minimum holds (720 − 320 − 1 = 399 for the
    // sidebar), and the stored width comes back.
    drag_divider(&mut shell, "sidebar", 600.);
    assert_eq!(sidebar_width(&mut shell), 480.);
    resize_window(&mut shell, 720., 800.);
    assert_eq!(sidebar_width(&mut shell), 399.);
    assert_eq!(bounds(shell.cx, "main-column").size.width, px(320.));
    resize_window(&mut shell, 1_200., 800.);
    assert_eq!(sidebar_width(&mut shell), 480.);
    drag_divider(&mut shell, "sidebar", 280.);

    // The threads panel: its pane (the divider its first point) follows the
    // pointer from the window's right edge, within 220 and what leaves the
    // viewport 260 of the main column (1,200 − 281 − 260 = 659).
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let pane = |shell: &mut Shell| bounds(shell.cx, "threads-pane").size.width.as_f32();
    assert_eq!(pane(&mut shell), 340.);
    assert_eq!(bounds(shell.cx, "threads-drag").center().x, px(860.5));
    for (to, width) in [(880., 320.), (1_100., 220.), (100., 659.)] {
        drag_divider(&mut shell, "threads", to);
        assert_eq!(pane(&mut shell), width, "dragged to {to}");
        assert_eq!(
            bounds(shell.cx, "viewport-pane").right(),
            px(1_200. - width),
            "the viewport ends at its divider"
        );
    }
    assert_eq!(bounds(shell.cx, "viewport-pane").size.width, px(260.));
    drag_divider(&mut shell, "threads", 880.);
    resize_window(&mut shell, 1_000., 800.);
    assert_eq!(pane(&mut shell), 320., "a window resize keeps it");
    resize_window(&mut shell, 720., 800.);
    assert_eq!(pane(&mut shell), 220., "the viewport keeps 260");
    assert_eq!(bounds(shell.cx, "viewport-pane").size.width, px(260.));
    resize_window(&mut shell, 1_200., 800.);
    assert_eq!(pane(&mut shell), 320., "and it comes back");
}

#[gpui_kit::test]
fn stored_width_survives_the_motion(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    drag_divider(&mut shell, "sidebar", 320.);
    assert_eq!(sidebar_width(&mut shell), 320.);
    click_now(&mut shell, "toggle-sidebar");
    finish(&mut shell);
    assert_eq!(frame_width(&mut shell), 0.0);
    click_now(&mut shell, "show-sidebar");
    finish(&mut shell);
    assert_eq!(sidebar_width(&mut shell), 320., "W at settle");
    assert_eq!(frame_width(&mut shell), 321.0);
}

#[gpui_kit::test]
fn the_divider_takes_no_drag_while_moving(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    set_at_once(&mut shell, false);
    click_now(&mut shell, "show-sidebar");
    let f = sample_at(&mut shell, 60.0);
    assert!(f > 100.0 && f < 281.0, "{f}");
    assert!(
        painted(shell.cx, "sidebar-drag").is_none(),
        "no band while it moves"
    );
    // A press on the divider's painted point, dragged 40 pt.
    let at = point(px(f - 0.5), px(400.));
    shell
        .cx
        .simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    for dx in [6., 40.] {
        shell.cx.simulate_mouse_move(
            point(at.x + px(dx), at.y),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        advance(shell.cx, FIRST_STEP);
        assert_eq!(frame_width(&mut shell), f, "frozen while pressed");
    }
    assert_eq!(requested_frames(shell.cx), 0);
    shell.cx.simulate_mouse_up(
        point(at.x + px(40.), at.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    frame(shell.cx);
    assert_eq!(frame_width(&mut shell), 281.0, "settled at the mouse up");
    assert_eq!(sidebar_width(&mut shell), 280.0, "W unchanged");
}

/// The main column's x, the divider's x and the content's x.
fn rest_edges(shell: &mut Shell) -> (f32, Option<f32>, Option<f32>) {
    (
        frame_width(shell),
        painted(shell.cx, "sidebar-divider").map(|b| b.left().as_f32()),
        painted(shell.cx, "sidebar").map(|b| b.left().as_f32()),
    )
}

#[gpui_kit::test]
fn settle_matches_rest(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    let shown = rest_edges(&mut shell);
    assert_eq!(shown, (281.0, Some(280.0), Some(0.0)));
    set_at_once(&mut shell, false);
    let hidden = rest_edges(&mut shell);
    assert_eq!(hidden, (0.0, None, None));
    set_at_once(&mut shell, true);
    for (name, end, rest) in [
        ("toggle-sidebar", CLOSE_MS, hidden),
        ("show-sidebar", OPEN_MS, shown),
    ] {
        click_now(&mut shell, name);
        step_to(shell.cx, ms(end - 1.0));
        assert_eq!(requested_frames(shell.cx), 1, "{name}: still moving");
        advance(shell.cx, ms(1.0));
        assert_eq!(rest_edges(&mut shell), rest, "{name}: the last frame");
        frame(shell.cx);
        assert_eq!(rest_edges(&mut shell), rest, "{name}: settled");
    }
}

#[gpui_kit::test]
fn retoggle_retargets_from_the_sampled_width(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    // The close after an open reversed at ½ lasts 180 · SLIDE(½).
    let reversed = CLOSE_MS * slide(0.5);
    for by_key in [false, true] {
        set_at_once(&mut shell, false);
        click_now(&mut shell, "show-sidebar");
        let sampled = sample_at(&mut shell, OPEN_MS / 2.0);
        assert_eq!(sampled, q(WBAR * slide(0.5)));
        if by_key {
            shell.cx.simulate_keystrokes("ctrl-cmd-s");
            frame(shell.cx);
        } else {
            click_now(&mut shell, "toggle-sidebar");
        }
        assert!(!visible(&mut shell), "by key {by_key}: hidden");
        assert_eq!(
            frame_width(&mut shell),
            sampled,
            "by key {by_key}: the close starts from the sampled f"
        );
        step_to(shell.cx, ms(reversed / 2.0));
        assert_eq!(
            frame_width(&mut shell),
            q(WBAR * slide(0.5) * (1.0 - slide(0.5))),
            "by key {by_key}: half of 180 · SLIDE(½)"
        );
        advance(shell.cx, ms(reversed / 2.0 + 0.5));
        assert_eq!(frame_width(&mut shell), 0.0, "by key {by_key}: closed");
        assert_eq!(requested_frames(shell.cx), 0, "by key {by_key}: settled");
    }
}

#[gpui_kit::test]
fn pointer_movement_does_not_settle_the_slide(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    set_at_once(&mut shell, false);
    click_now(&mut shell, "show-sidebar");
    step_to(shell.cx, ms(60.0));
    for x in [100., 400., 900.] {
        shell
            .cx
            .simulate_mouse_move(point(px(x), px(300.)), None, Modifiers::none());
    }
    advance(shell.cx, ms(60.0));
    assert_eq!(
        frame_width(&mut shell),
        q(WBAR * slide(0.5)),
        "still on its way at ½"
    );
}

#[gpui_kit::test]
fn resize_and_fullscreen_settle_the_motion(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    set_at_once(&mut shell, false);
    click_now(&mut shell, "show-sidebar");
    step_to(shell.cx, ms(60.0));
    assert!(frame_width(&mut shell) < 281.0);
    shell.cx.simulate_resize(size(px(1180.), px(800.)));
    frame(shell.cx);
    assert_eq!(frame_width(&mut shell), 281.0, "a resize settles it");
    assert_eq!(requested_frames(shell.cx), 0);

    click_now(&mut shell, "toggle-sidebar");
    step_to(shell.cx, ms(45.0));
    assert!(frame_width(&mut shell) > 0.0);
    // Entering fullscreen changes the content size (the test platform's
    // fullscreen does not resize the window, so the test does).
    shell.cx.update(|window, _| window.toggle_fullscreen());
    shell.cx.simulate_resize(size(px(1440.), px(900.)));
    frame(shell.cx);
    assert_eq!(frame_width(&mut shell), 0.0, "fullscreen settles it");
    assert_eq!(requested_frames(shell.cx), 0);
}

#[gpui_kit::test]
fn ctrl_cmd_s_and_palette_animate_per_oq_58(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    shell.cx.simulate_keystrokes("ctrl-cmd-s");
    frame(shell.cx);
    assert!(!visible(&mut shell));
    assert_eq!(frame_width(&mut shell), 281.0, "⌃⌘S: the commit frame");
    assert_eq!(
        sample_at(&mut shell, CLOSE_MS / 2.0),
        q(WBAR * (1.0 - slide(0.5))),
        "⌃⌘S slides it"
    );
    advance(shell.cx, ms(CLOSE_MS));

    // The palette's Toggle sidebar, chosen with the keyboard.
    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    shell.cx.simulate_input("toggle sidebar");
    draw(shell.cx);
    shell.cx.simulate_keystrokes("enter");
    frame(shell.cx);
    assert!(visible(&mut shell));
    assert_eq!(
        frame_width(&mut shell),
        0.0,
        "the palette: the commit frame"
    );
    assert_eq!(
        sample_at(&mut shell, OPEN_MS / 2.0),
        q(WBAR * slide(0.5)),
        "the palette slides it"
    );
}

#[gpui_kit::test]
fn show_files_and_restore_snap(cx: &mut TestAppContext) {
    use polygloss_app::keymap::actions::tree::FocusFilter;
    use polygloss_app::keymap::actions::window::{ShowFiles, ShowReviews};
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    let snapped = |shell: &mut Shell, how: &str| {
        assert!(visible(shell), "{how}: shown");
        assert_eq!(
            frame_width(shell),
            281.0,
            "{how}: the commit frame is the end"
        );
        assert_eq!(requested_frames(shell.cx), 0, "{how}: nothing runs");
    };
    let reset = |shell: &mut Shell| {
        shell.cx.simulate_keystrokes("escape");
        draw(shell.cx);
        set_at_once(shell, false);
        shell.cx.update(|window, cx| window.focus(&viewport, cx));
        draw(shell.cx);
    };
    reset(&mut shell);
    for (how, keys) in [("⌘F", "cmd-f"), ("⌘P", "cmd-p")] {
        shell.cx.simulate_keystrokes(keys);
        frame(shell.cx);
        snapped(&mut shell, how);
        reset(&mut shell);
    }
    // `⇥` into the tree: the pane cycle skips a hidden sidebar, so it
    // reaches the tree only through `focus_pane`, which shows it first.
    tab.update_in(shell.cx, |t, window, cx| {
        keyboard::focus_pane(t, &Pane::Tree, window, cx)
    });
    frame(shell.cx);
    snapped(&mut shell, "⇥");
    reset(&mut shell);
    // `/` (the filter), ShowFiles and ShowReviews, as dispatched.
    for (how, action) in [
        ("/", Box::new(FocusFilter) as Box<dyn gpui_kit::Action>),
        ("ShowFiles", Box::new(ShowFiles)),
        ("ShowReviews", Box::new(ShowReviews)),
    ] {
        shell
            .cx
            .update(|window, cx| viewport.dispatch_action(action.as_ref(), window, cx));
        frame(shell.cx);
        snapped(&mut shell, how);
        reset(&mut shell);
    }
}

/// The alphas of the quads painted in the sidebar's color, relative to
/// that color's: the frame's opaque fill and the content's root.
fn sidebar_fills(shell: &mut Shell) -> Vec<f32> {
    use gpui_kit::component::ActiveTheme as _;
    shell.cx.update(|window, cx| {
        let fill: Hsla = cx.theme().sidebar;
        window
            .painted_quads()
            .into_iter()
            .filter_map(|q| q.background.as_solid())
            .filter(|c| c.h == fill.h && c.s == fill.s && c.l == fill.l && c.a > 0.0)
            .map(|c| c.a / fill.a)
            .collect()
    })
}

/// The tree's filter field's fill alpha, relative to its color's: the
/// content's ink.
fn filter_ink(shell: &mut Shell) -> Option<f32> {
    use gpui_kit::component::ActiveTheme as _;
    shell.cx.update(|window, cx| {
        let fill: Hsla = cx.theme().tab_bar_segmented;
        window
            .painted_quads()
            .into_iter()
            .filter_map(|q| q.background.as_solid())
            .find(|c| c.h == fill.h && c.s == fill.s && c.l == fill.l && c.a > 0.0)
            .map(|c| c.a / fill.a)
    })
}

fn strictly_between(v: f32) -> bool {
    0.0 < v && v < 1.0
}

#[gpui_kit::test]
fn reduced_open_fades_in_the_held_frame(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    set_policy(&mut shell, MotionPolicy::Reduced);
    set_at_once(&mut shell, false);
    click_now(&mut shell, "show-sidebar");
    assert_eq!(frame_width(&mut shell), 281.0, "the frame is W̄ at once");
    assert_eq!(bounds(shell.cx, "sidebar").left(), px(0.), "zero travel");
    step_to(shell.cx, FIRST_STEP);
    assert_eq!(frame_width(&mut shell), 281.0);
    advance(shell.cx, ms(75.0) - FIRST_STEP);
    assert_eq!(frame_width(&mut shell), 281.0, "75 ms: no travel");
    assert_eq!(bounds(shell.cx, "sidebar").left(), px(0.));
    let fills = sidebar_fills(&mut shell);
    assert!(
        fills.contains(&1.0),
        "the frame's fill is opaque: {fills:?}"
    );
    assert!(
        fills.iter().any(|a| strictly_between(*a)),
        "the content fades in: {fills:?}"
    );
    let ink = filter_ink(&mut shell).expect("the filter field is painted");
    assert!(strictly_between(ink), "the tree's ink at 75 ms: {ink}");
    advance(shell.cx, ms(80.0));
    assert!(sidebar_fills(&mut shell).iter().all(|a| *a == 1.0));
    assert_eq!(filter_ink(&mut shell), Some(1.0), "opaque at settle");
    assert_eq!(requested_frames(shell.cx), 0);
}

#[gpui_kit::test]
fn reduced_close_fades_then_snaps(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    // A width where a viewport held at the hidden width would turn split.
    resize_window(&mut shell, FLIPS, 800.);
    set_policy(&mut shell, MotionPolicy::Reduced);
    let (layout, relayouts) = viewport_layout(&mut shell, &tab);
    assert_eq!(layout, Layout::Unified);
    click_now(&mut shell, "toggle-sidebar");
    assert!(!visible(&mut shell));
    step_to(shell.cx, ms(50.0));
    assert_eq!(frame_width(&mut shell), 281.0, "the frame holds W̄");
    let fills = sidebar_fills(&mut shell);
    assert!(
        fills.iter().any(|a| strictly_between(*a)),
        "the content fades out at 50 ms: {fills:?}"
    );
    assert_eq!(
        viewport_layout(&mut shell, &tab),
        (layout, relayouts),
        "the main column unmoved and the viewport not held"
    );
    advance(shell.cx, ms(55.0));
    frame(shell.cx);
    assert_eq!(frame_width(&mut shell), 0.0, "then the width snaps");
    assert_eq!(
        viewport_layout(&mut shell, &tab).0,
        Layout::Split,
        "the viewport reshapes once, at the snap"
    );
}

#[gpui_kit::test]
fn off_settles(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, _tab) = review(cx);
    set_policy(&mut shell, MotionPolicy::Off);
    click_now(&mut shell, "toggle-sidebar");
    assert_eq!(frame_width(&mut shell), 0.0, "settled on the commit frame");
    assert_eq!(requested_frames(shell.cx), 0);
    click_now(&mut shell, "show-sidebar");
    assert_eq!(frame_width(&mut shell), 281.0);
    assert_eq!(requested_frames(shell.cx), 0);
}

/// Delivers the frame motion requested and draws, without refreshing the
/// window (a refresh renders cached views again).
fn deliver(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn tree_renders_uncached_while_moving_and_cached_at_rest(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    let tree = tab.read_with(shell.cx, |t, _| file_tree(t).cloned().expect("a tree"));
    let renders = |shell: &mut Shell| tree.read_with(shell.cx, |t, _| t.render_count());
    // Reduced too: a fade keeps the tree's bounds, where only rendering it
    // uncached shows each frame's opacity.
    for policy in [MotionPolicy::Full, MotionPolicy::Reduced] {
        set_policy(&mut shell, policy);
        set_at_once(&mut shell, false);
        click_now(&mut shell, "show-sidebar");
        let mut last = renders(&mut shell);
        let mut frames = 0;
        let mut at = Duration::ZERO;
        while shell.cx.update(|_, cx| {
            use polygloss_app::panel_split::PanelOwner as _;
            chrome::chrome(cx).read(cx).panel().moving()
        }) {
            let step = if at.is_zero() { FIRST_STEP } else { ms(16.0) };
            at += step;
            deliver(shell.cx, step);
            let now = renders(&mut shell);
            assert_eq!(now, last + 1, "{policy:?}: one render at {at:?}");
            last = now;
            frames += 1;
        }
        assert!(frames >= 9, "{policy:?}: {frames} motion frames");
        // A settled frame without a change: the tab draws, the tree is
        // reused.
        tab.update(shell.cx, |_, cx| cx.notify());
        shell.cx.run_until_parked();
        assert_eq!(renders(&mut shell), last, "{policy:?}: cached at rest");
    }
}
