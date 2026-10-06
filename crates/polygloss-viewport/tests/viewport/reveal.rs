//! The card reveal (T7.8, ADR-0030 M3 and "Viewport reveals"): a pointer
//! collapse or expand of a file card glides, the card's frame and chevron
//! following, later cards riding below it; every other path snaps; and
//! `hold_layout` keeps what reshapes at a target width while a panel moves.
//!
//! Fixture: `a.rs`'s card grows by 400 pt when it opens (a gap row of 32,
//! 18 rows of 20 and the card's 8 pt bottom padding, which a collapsed card
//! does not have), in a 900 pt viewport at scale 2. Expected values are this
//! file's own: durations written by hand from ADR-0030's tokens, the SLIDE
//! curve by its own bisection solver, each y quantized here as
//! `round(v · 2) / 2`. Tests step the clock by ADR-0030's stepping protocol:
//! the commit frame is t = 0, the first step is exactly 16.667 ms.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    IntoElement as _, Modifiers, ParentElement as _, Styled as _, TestAppContext,
    VisualTestContext, div, point, px, size,
};
use polygloss_diff::Side;
use polygloss_viewport::motion::{Initiator, MotionPolicy, MotionPolicyOverride, Settle};
use polygloss_viewport::{
    BlockAnchor, BlockId, BlockSpec, ControlAction, DiffViewport, LayoutMode, ScrollTarget,
    ViewportDebug,
};

use crate::support::*;

/// The viewport's height and the window's scale.
const VIEW_H: f32 = 900.0;
const SCALE: f32 = 2.0;
/// ADR-0031's numbers: a card's bottom padding and the gap between cards.
const CARD_Y: f32 = 8.0;
const CARDS: f32 = 12.0;
/// How far `a.rs`'s card grows when it opens.
const GROWTH: f32 = 400.0;
/// The first step after a commit (ADR-0030 `FIRST_STEP`).
const FIRST_STEP: Duration = Duration::from_micros(16_667);

fn ms(ms: f64) -> Duration {
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
fn slide(t: f64) -> f64 {
    bezier(0.25, 1.0, 0.5, 1.0, t)
}

/// `v` on the device pixel grid at scale 2.
fn q(v: f64) -> f32 {
    ((v * f64::from(SCALE)).round() / f64::from(SCALE)) as f32
}

fn assert_y(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "{what}: painted at {actual}, expected {expected}"
    );
}

/// `a.rs`: 30 lines, then 15 appended (a 27-line gap row, 3 context rows
/// and 15 added rows: 392 pt of rows); `b.rs` and `c.rs`: added files.
fn fixture(c_lines: u32) -> std::sync::Arc<MemProvider> {
    let old = numbered("a", 30).concat();
    let new = old.clone() + &numbered("added", 15).concat();
    MemProvider::new(vec![
        Spec::modified("a.rs", &old, &new),
        Spec::added("b.rs", &numbered("b", 10).concat()),
        Spec::added("c.rs", &numbered("c", c_lines).concat()),
    ])
}

/// A window `width` wide showing `provider` in cards, unified, settled,
/// under `policy`.
fn open_cards(
    cx: &mut TestAppContext,
    provider: std::sync::Arc<MemProvider>,
    width: f32,
) -> (gpui_kit::Entity<DiffViewport>, &mut VisualTestContext) {
    open(
        cx,
        provider,
        card_options(LayoutMode::Unified),
        width,
        VIEW_H,
    )
}

fn set_policy(cx: &mut VisualTestContext, policy: MotionPolicy) {
    cx.update(|_, cx| cx.set_global(MotionPolicyOverride(Some(policy))));
}

/// Draws one frame at the clock's time, delivering the frames motion
/// requested (their owners are notified, as a display frame would).
fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.refresh();
    });
    cx.run_until_parked();
}

fn advance(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    frame(cx);
}

/// From the commit frame, draws the frame `since_commit` later by the
/// stepping protocol.
fn step_to(cx: &mut VisualTestContext, since_commit: Duration) {
    let first = since_commit.min(FIRST_STEP);
    advance(cx, first);
    if since_commit > first {
        advance(cx, since_commit - first);
    }
}

/// Frames requested since the last one was delivered (delivering them).
fn requested_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

/// A press and release at viewport `(x, y)`: the commit frame is drawn at
/// the clock's time, nothing later.
fn click(cx: &mut VisualTestContext, (x, y): (f32, f32)) {
    cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
    cx.simulate_click(point(px(x), px(y)), Modifiers::default());
}

fn center((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

/// Clicks file `f`'s painted chevron.
fn click_chevron(view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext, f: u32) {
    let chevron = control(&debug(view, cx), ControlAction::Collapse(f));
    click(cx, center(chevron));
}

/// File `f`'s header as painted.
fn header_y(d: &ViewportDebug, f: u32) -> f32 {
    d.headers
        .iter()
        .find(|h| h.file_idx == f)
        .unwrap_or_else(|| panic!("no header for file {f}: {:?}", d.headers))
        .y
}

/// File `f`'s card as painted, `(x, y, w, h)`.
fn card(d: &ViewportDebug, f: u32) -> (f32, f32, f32, f32) {
    d.cards
        .iter()
        .find(|c| c.file_idx == f)
        .unwrap_or_else(|| panic!("no card for file {f}: {:?}", d.cards))
        .bounds
}

fn chevron_angle(d: &ViewportDebug, f: u32) -> f32 {
    d.chevrons
        .iter()
        .find(|(file, _)| *file == f)
        .unwrap_or_else(|| panic!("no chevron for file {f}: {:?}", d.chevrons))
        .1
}

fn running(view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext) -> bool {
    view.read_with(cx, |v, _| v.motion_running())
}

/// `a.rs` collapsed under Off (a snap), then `policy` for what follows.
fn collapsed_a(
    cx: &mut TestAppContext,
    policy: MotionPolicy,
) -> (gpui_kit::Entity<DiffViewport>, &mut VisualTestContext) {
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    view.update(cx, |v, cx| v.set_collapsed(0, true, cx));
    settle(cx);
    set_policy(cx, policy);
    (view, cx)
}

/// Where `b.rs`'s header is with `a.rs` collapsed (its card is its 46 pt
/// header) and open.
const B_CLOSED: f32 = 46.0 + CARDS;
const B_OPEN: f32 = B_CLOSED + GROWTH;

#[gpui_kit::test]
fn pointer_expand_reveals_over_time(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    assert_eq!(header_y(&debug(&view, cx), 1), B_CLOSED);
    click_chevron(&view, cx, 0);
    // reveal(400 pt) = 150 + 40 = 190 ms.
    let total = 190.0;
    assert!(!view.read_with(cx, |v, _| v.document().is_collapsed(0)));
    assert_y(header_y(&debug(&view, cx), 1), B_CLOSED, "commit frame");
    assert!(running(&view, cx));
    step_to(cx, FIRST_STEP);
    let first = f64::from(B_CLOSED) + f64::from(GROWTH) * slide(16.667 / total);
    assert_y(header_y(&debug(&view, cx), 1), q(first), "first step");
    advance(cx, ms(95.0) - FIRST_STEP);
    let half = f64::from(B_CLOSED) + f64::from(GROWTH) * slide(0.5);
    assert_y(header_y(&debug(&view, cx), 1), q(half), "95 ms");
    advance(cx, ms(95.0));
    assert_y(header_y(&debug(&view, cx), 1), B_OPEN, "190 ms");
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn pointer_collapse_reveals_over_time(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Full);
    assert_eq!(header_y(&debug(&view, cx), 1), B_OPEN);
    click_chevron(&view, cx, 0);
    assert!(view.read_with(cx, |v, _| v.document().is_collapsed(0)));
    assert_y(header_y(&debug(&view, cx), 1), B_OPEN, "commit frame");
    // exit(190 ms) = 140 ms.
    step_to(cx, ms(70.0));
    let half = f64::from(B_OPEN) - f64::from(GROWTH) * slide(0.5);
    assert_y(header_y(&debug(&view, cx), 1), q(half), "70 ms");
    advance(cx, ms(70.0));
    assert_y(header_y(&debug(&view, cx), 1), B_CLOSED, "140 ms");
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn card_frame_follows_the_curtain(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Full);
    let check = |d: &ViewportDebug, what: &str| {
        let reveal = d.reveal.unwrap_or_else(|| panic!("{what}: no reveal"));
        let (_, y, _, h) = card(d, 0);
        let bottom = y + h;
        assert_y(bottom - CARD_Y, reveal.curtain, &format!("{what}: curtain"));
        assert_y(card(d, 1).1, bottom + CARDS, &format!("{what}: next card"));
    };
    click_chevron(&view, cx, 0);
    check(&debug(&view, cx), "collapse, commit");
    step_to(cx, FIRST_STEP);
    check(&debug(&view, cx), "collapse, first step");
    advance(cx, ms(70.0) - FIRST_STEP);
    check(&debug(&view, cx), "collapse, 70 ms");
    advance(cx, ms(70.0));
    // Settled, collapsed: the card is its header, the next card 12 below.
    let d = debug(&view, cx);
    assert_eq!(card(&d, 0).3, 46.0);
    assert_eq!(card(&d, 1).1, B_CLOSED);
    click_chevron(&view, cx, 0);
    step_to(cx, FIRST_STEP);
    check(&debug(&view, cx), "expand, first step");
    advance(cx, ms(95.0) - FIRST_STEP);
    check(&debug(&view, cx), "expand, 95 ms");
}

#[gpui_kit::test]
fn travel_is_clamped_to_the_viewport(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // a.rs: 250 added lines, a 5,000 pt body (5,008 with its padding).
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 250).concat()),
        Spec::added("b.rs", &numbered("b", 10).concat()),
    ]);
    let (view, cx) = open_cards(cx, provider, 1000.);
    set_policy(cx, MotionPolicy::Full);
    let body_top = 46.0;
    click_chevron(&view, cx, 0);
    // reveal(5,008 pt in 900) = 200 ms; exit(200) = 150 ms.
    let frame_bottom = |d: &ViewportDebug| {
        let (_, y, _, h) = card(d, 0);
        y + h
    };
    let d = debug(&view, cx);
    assert_y(frame_bottom(&d), body_top + VIEW_H, "commit frame");
    assert_y(
        d.reveal.unwrap().curtain,
        body_top + VIEW_H - CARD_Y,
        "curtain",
    );
    step_to(cx, ms(75.0));
    let d = debug(&view, cx);
    let at = f64::from(body_top) + f64::from(VIEW_H) * (1.0 - slide(0.5));
    assert_y(frame_bottom(&d), q(at), "75 ms");
    assert_y(card(&d, 1).1, frame_bottom(&d) + CARDS, "next card, 75 ms");
    advance(cx, ms(75.0));
    assert!(!running(&view, cx));
    // The expand: from the header, 900 · SLIDE(½) below it at 100 ms.
    click_chevron(&view, cx, 0);
    let d = debug(&view, cx);
    assert_y(frame_bottom(&d), body_top, "expand, commit frame");
    assert_y(d.reveal.unwrap().curtain, body_top, "expand, curtain");
    step_to(cx, ms(100.0));
    let d = debug(&view, cx);
    let at = f64::from(body_top) + f64::from(VIEW_H) * slide(0.5);
    assert_y(frame_bottom(&d), q(at), "expand, 100 ms");
    assert_y(card(&d, 1).1, frame_bottom(&d) + CARDS, "expand, next card");
    advance(cx, ms(100.0));
    assert!(!running(&view, cx));
    // b.rs is at its committed y, 5,066 pt down: below the viewport.
    assert!(debug(&view, cx).slots.iter().all(|&(f, _)| f != 1));
}

/// File `f`'s card top as painted (its slot's y).
fn slot_y(d: &ViewportDebug, f: u32) -> f32 {
    d.slots
        .iter()
        .find(|(file, _)| *file == f)
        .unwrap_or_else(|| panic!("no slot for file {f}: {:?}", d.slots))
        .1
}

#[gpui_kit::test]
fn rows_keep_their_screen_y(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Full);
    let before = debug(&view, cx);
    let rows_of_a = |d: &ViewportDebug| -> Vec<(String, f32)> {
        let b_top = header_y(d, 1);
        d.visible_rows
            .iter()
            .zip(&d.row_bounds)
            .filter(|(text, (y, _))| !text.starts_with("==") && *y < b_top)
            .map(|(text, (y, _))| (text.clone(), *y))
            .collect()
    };
    let rows = rows_of_a(&before);
    assert_eq!(rows.len(), 19, "{rows:?}");
    click_chevron(&view, cx, 0);
    step_to(cx, ms(70.0));
    let d = debug(&view, cx);
    let curtain = d.reveal.expect("revealing").curtain;
    let painted = rows_of_a(&d);
    assert!(!painted.is_empty(), "rows under the curtain are painted");
    for (text, y) in &painted {
        assert!(
            *y < curtain,
            "{text} painted at {y}, below the curtain {curtain}"
        );
        let was = rows.iter().find(|(t, _)| t == text).map(|(_, y)| *y);
        assert_eq!(was, Some(*y), "{text} moved");
    }
}

#[gpui_kit::test]
fn pinned_header_collapse_rises_from_the_bottom(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // a.rs: 200 added lines (a 4,000 pt body), scrolled 3,000 pt past.
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 200).concat()),
        Spec::added("b.rs", &numbered("b", 10).concat()),
    ]);
    let (view, cx) = open_cards(cx, provider, 1000.);
    wheel(cx, 3_000.0);
    set_policy(cx, MotionPolicy::Full);
    let before = debug(&view, cx);
    let h = before.headers.iter().find(|h| h.file_idx == 0).unwrap();
    assert!(h.sticky && h.y == 0.0, "{h:?}");
    let visible: Vec<(String, f32)> = before
        .visible_rows
        .iter()
        .zip(&before.row_bounds)
        .filter(|(t, (y, _))| !t.starts_with("==") && *y >= 46.0)
        .map(|(t, (y, _))| (t.clone(), *y))
        .collect();
    click_chevron(&view, cx, 0);
    let check = |d: &ViewportDebug, what: &str| {
        let h = d.headers.iter().find(|h| h.file_idx == 0).unwrap();
        assert_eq!(h.y, 0.0, "{what}: the header at the viewport's top");
        let curtain = d.reveal.expect("revealing").curtain;
        for (text, y) in d.visible_rows.iter().zip(&d.row_bounds) {
            if text.starts_with("==") || y.0 >= curtain || !text.contains(" a ") {
                continue;
            }
            let was = visible.iter().find(|(t, _)| t == text).map(|(_, y)| *y);
            assert_eq!(was, Some(y.0), "{what}: {text} moved");
        }
        let (_, y, _, h) = card(d, 0);
        y + h
    };
    // The commit: the frame 900 below the header (the clamp), so the next
    // card starts at the viewport's bottom.
    let commit = check(&debug(&view, cx), "commit frame");
    assert!(commit + CARDS >= VIEW_H, "the next card below: {commit}");
    step_to(cx, FIRST_STEP);
    let first = check(&debug(&view, cx), "first step");
    assert!(first < commit, "the frame closes from the first step");
    advance(cx, ms(75.0) - FIRST_STEP);
    let d = debug(&view, cx);
    let half = check(&d, "75 ms");
    assert_y(slot_y(&d, 1), half + CARDS, "75 ms: the next card");
    assert!(slot_y(&d, 1) < VIEW_H, "75 ms: the next card rose");
}

#[gpui_kit::test]
fn pinned_collapse_reversed_puts_the_rows_back(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 200).concat()),
        Spec::added("b.rs", &numbered("b", 10).concat()),
    ]);
    let (view, cx) = open_cards(cx, provider, 1000.);
    wheel(cx, 3_000.0);
    set_policy(cx, MotionPolicy::Full);
    let before = debug(&view, cx);
    click_chevron(&view, cx, 0);
    step_to(cx, ms(40.0));
    let painted = debug(&view, cx);
    // The second click reopens from where it is: the rows on screen stay.
    click_chevron(&view, cx, 0);
    assert!(!view.read_with(cx, |v, _| v.document().is_collapsed(0)));
    let reopened = debug(&view, cx);
    assert_eq!(reopened.visible_rows, painted.visible_rows);
    assert_eq!(reopened.row_bounds, painted.row_bounds);
    step_to(cx, ms(200.0));
    assert!(!running(&view, cx));
    // Settled, the header is pinned over the same rows as before the first
    // click: nothing jumps at the end.
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows, before.visible_rows);
    assert_eq!(after.row_bounds, before.row_bounds);
}

#[gpui_kit::test]
fn the_model_commits_on_the_first_frame(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    let model = |view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            let doc = v.document();
            (
                doc.total_height(),
                doc.max_scroll(),
                doc.file_at_offset(500.0),
            )
        })
    };
    // Where the model ends, from a snap (Off).
    set_policy(cx, MotionPolicy::Off);
    click_chevron(&view, cx, 0);
    let settled = model(&view, cx);
    click_chevron(&view, cx, 0);
    set_policy(cx, MotionPolicy::Full);
    click_chevron(&view, cx, 0);
    assert!(running(&view, cx));
    assert_eq!(model(&view, cx), settled, "the commit frame");
    step_to(cx, ms(70.0));
    assert_eq!(model(&view, cx), settled, "mid-motion");
}

#[gpui_kit::test]
fn hit_testing_follows_the_painted_frame(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    click_chevron(&view, cx, 0);
    step_to(cx, ms(95.0));
    let d = debug(&view, cx);
    let chevron = control(&d, ControlAction::Collapse(1));
    assert!(chevron.1 > B_CLOSED && chevron.1 < B_OPEN, "{chevron:?}");
    click(cx, center(chevron));
    assert!(view.read_with(cx, |v, _| v.document().is_collapsed(1)));
    assert!(!view.read_with(cx, |v, _| v.document().is_collapsed(0)));
}

#[gpui_kit::test]
fn keyboard_z_and_set_collapsed_snap(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Full);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.toggle_collapsed_by(0, Initiator::Keyboard, window, cx)
        })
    });
    assert!(!running(&view, cx));
    assert_eq!(header_y(&debug(&view, cx), 1), B_CLOSED);
    assert_eq!(requested_frames(cx), 0);
    view.update(cx, |v, cx| v.set_collapsed(0, false, cx));
    frame(cx);
    assert!(!running(&view, cx));
    assert_eq!(header_y(&debug(&view, cx), 1), B_OPEN);
    assert_eq!(requested_frames(cx), 0);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.toggle_collapsed_by(0, Initiator::Programmatic, window, cx)
        })
    });
    assert!(!running(&view, cx));
    assert_eq!(header_y(&debug(&view, cx), 1), B_CLOSED);
}

/// The veil over `a.rs`'s rows during a Reduced fade: its alpha, if one is
/// painted in the card's background over them.
fn veil_alpha(view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext) -> Option<f32> {
    let background = view.read_with(cx, |v, _| v.options().theme.card_background);
    let rows_top = 46.0;
    shaped_quads(cx)
        .into_iter()
        .filter_map(|q| {
            let fill = q.fill?;
            let (_, y, _, h) = q.bounds;
            let same = fill.h == background.h && fill.s == background.s && fill.l == background.l;
            (same && fill.a < background.a && y <= rows_top && y + h > rows_top + 300.0)
                .then_some(fill.a / background.a)
        })
        .next()
}

#[gpui_kit::test]
fn reduced_expand_fades_rows_without_displacement(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Reduced);
    click_chevron(&view, cx, 0);
    let rows_open = |d: &ViewportDebug| {
        d.visible_rows
            .iter()
            .zip(&d.row_bounds)
            .find(|(t, _)| t.contains("added 0"))
            .map(|(_, b)| b.0)
    };
    let check = |d: &ViewportDebug, what: &str| {
        assert_y(header_y(d, 1), B_OPEN, &format!("{what}: no displacement"));
        assert_eq!(chevron_angle(d, 0), 0.0, "{what}: no rotation");
        assert!(rows_open(d).is_some(), "{what}: the rows are in place");
    };
    check(&debug(&view, cx), "commit frame");
    step_to(cx, FIRST_STEP);
    check(&debug(&view, cx), "first step");
    advance(cx, ms(75.0) - FIRST_STEP);
    check(&debug(&view, cx), "75 ms");
    // Half of QUICK: the rows strictly between transparent and opaque.
    let veil = veil_alpha(&view, cx).expect("a fade over the rows");
    assert!(veil > 0.0 && veil < 1.0, "veil {veil}");
    advance(cx, ms(75.0));
    assert_eq!(veil_alpha(&view, cx), None);
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn reduced_collapse_fades_then_snaps(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Reduced);
    click_chevron(&view, cx, 0);
    let d = debug(&view, cx);
    assert_y(header_y(&d, 1), B_OPEN, "commit frame");
    // No rotation: the chevron shows the closed state from the commit.
    assert_eq!(chevron_angle(&d, 0), -90.0, "commit frame");
    step_to(cx, ms(50.0));
    let d = debug(&view, cx);
    assert_y(header_y(&d, 1), B_OPEN, "50 ms: the frame holds");
    assert_eq!(chevron_angle(&d, 0), -90.0, "50 ms");
    let veil = veil_alpha(&view, cx).expect("a fade over the rows");
    assert!(veil > 0.0 && veil < 1.0, "veil {veil}");
    advance(cx, ms(50.0));
    assert_y(header_y(&debug(&view, cx), 1), B_CLOSED, "100 ms: snapped");
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn off_settles_and_requests_no_frame(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Off);
    click_chevron(&view, cx, 0);
    assert!(!running(&view, cx));
    assert_eq!(header_y(&debug(&view, cx), 1), B_CLOSED);
    assert_eq!(requested_frames(cx), 0);
}

#[gpui_kit::test]
fn second_click_reverses_from_the_painted_height(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    click_chevron(&view, cx, 0);
    step_to(cx, ms(95.0));
    let painted = header_y(&debug(&view, cx), 1);
    click_chevron(&view, cx, 0);
    assert!(view.read_with(cx, |v, _| v.document().is_collapsed(0)));
    assert_y(
        header_y(&debug(&view, cx), 1),
        painted,
        "the reversal's commit",
    );
    // It closes over exit(190) = 140 ms × the share travelled, SLIDE(½).
    let total = 140.0 * slide(0.5);
    step_to(cx, ms(total / 2.0));
    let y = header_y(&debug(&view, cx), 1);
    assert!(y > B_CLOSED && y < painted, "between: {y}");
    advance(cx, ms(total / 2.0 + 1.0));
    assert_y(header_y(&debug(&view, cx), 1), B_CLOSED, "closed");
}

#[gpui_kit::test]
fn pointer_movement_does_not_settle_the_reveal(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    click_chevron(&view, cx, 0);
    step_to(cx, ms(47.5));
    for x in [100.0, 400.0, 700.0] {
        cx.simulate_mouse_move(point(px(x), px(300.)), None, Modifiers::default());
    }
    advance(cx, ms(47.5));
    let y = header_y(&debug(&view, cx), 1);
    assert!(y > B_CLOSED && y < B_OPEN, "still between at ½: {y}");
    assert!(running(&view, cx));
}

#[gpui_kit::test]
fn model_changes_settle_the_reveal(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    let start = |view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext| {
        click_chevron(view, cx, 0);
        step_to(cx, ms(60.0));
        assert!(running(view, cx));
    };
    let collapse_a = |view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext| {
        set_policy(cx, MotionPolicy::Off);
        view.update(cx, |v, cx| v.set_collapsed(0, true, cx));
        view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
        frame(cx);
        set_policy(cx, MotionPolicy::Full);
    };
    // A wheel step.
    start(&view, cx);
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: point(px(200.), px(200.)),
        delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-10.))),
        modifiers: Modifiers::default(),
        ..Default::default()
    });
    assert!(!running(&view, cx), "a wheel step settles");
    assert_y(
        header_y(&debug(&view, cx), 1),
        B_OPEN - 10.0,
        "settled, scrolled",
    );
    // A jump.
    collapse_a(&view, cx);
    start(&view, cx);
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    frame(cx);
    assert!(!running(&view, cx), "a jump settles");
    assert_y(header_y(&debug(&view, cx), 1), B_OPEN, "settled");
    // Another file's toggle.
    collapse_a(&view, cx);
    start(&view, cx);
    view.update(cx, |v, cx| v.set_collapsed(2, true, cx));
    frame(cx);
    assert!(!running(&view, cx), "another file's toggle settles");
    assert_y(header_y(&debug(&view, cx), 1), B_OPEN, "settled");
}

#[gpui_kit::test]
fn freeze_holds_paint_and_hitboxes(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    click_chevron(&view, cx, 0);
    step_to(cx, ms(60.0));
    let now = cx.update(|_, cx| cx.background_executor().now());
    view.update(cx, |v, cx| v.freeze_motion(now, cx));
    frame(cx);
    let held = debug(&view, cx);
    assert!(held.reveal.expect("revealing").frozen);
    for _ in 0..3 {
        assert_eq!(requested_frames(cx), 0, "no frame while frozen");
        advance(cx, FIRST_STEP);
    }
    let d = debug(&view, cx);
    assert_eq!(header_y(&d, 1), header_y(&held, 1), "paint holds");
    assert_eq!(
        control(&d, ControlAction::Collapse(1)),
        control(&held, ControlAction::Collapse(1)),
        "hitboxes hold"
    );
    view.update(cx, |v, cx| v.settle_motion(Settle::Frozen, cx));
    frame(cx);
    assert!(!running(&view, cx));
    assert_eq!(header_y(&debug(&view, cx), 1), B_OPEN, "settled at its end");

    // A reveal started after the freeze runs on through `Settle::Frozen`.
    click_chevron(&view, cx, 0);
    step_to(cx, ms(30.0));
    let now = cx.update(|_, cx| cx.background_executor().now());
    view.update(cx, |v, cx| v.freeze_motion(now, cx));
    click_chevron(&view, cx, 1);
    view.update(cx, |v, cx| v.settle_motion(Settle::Frozen, cx));
    frame(cx);
    assert!(running(&view, cx), "b.rs's reveal runs on");
    assert!(!debug(&view, cx).reveal.unwrap().frozen);
}

#[gpui_kit::test]
fn unloaded_body_snaps(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = fixture(10);
    let (view, cx) = open_idle(
        cx,
        provider,
        card_options(LayoutMode::Unified),
        1000.,
        VIEW_H,
    );
    // Collapsed before anything loaded: its data is not there.
    view.update(cx, |v, cx| v.set_collapsed(0, true, cx));
    redraw(cx);
    set_policy(cx, MotionPolicy::Full);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.toggle_collapsed_by(0, Initiator::Pointer, window, cx)
        })
    });
    assert!(!view.read_with(cx, |v, _| v.document().is_collapsed(0)));
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn a_body_collapsed_before_layout_animates(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    view.update(cx, |v, cx| v.set_collapsed(0, true, cx));
    settle(cx);
    // Another layout drops every row layout; a.rs stays loaded, collapsed.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    assert!(view.read_with(cx, |v, _| v.document().file_layout(0).is_none()));
    set_policy(cx, MotionPolicy::Full);
    let (events, _sub) = record_events(&view, cx);
    click_chevron(&view, cx, 0);
    assert!(running(&view, cx));
    assert!(view.read_with(cx, |v, _| v.document().file_layout(0).is_some()));
    let commit = all_stats(&events).len();
    step_to(cx, ms(95.0));
    advance(cx, ms(100.0));
    let stats = all_stats(&events);
    assert!(stats.len() > commit + 1, "frames after the commit");
    for s in &stats[commit..] {
        assert_eq!(s.shaped_lines, 0, "shaped after the commit frame: {s:?}");
    }
}

#[gpui_kit::test]
fn offsets_are_quantized(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = collapsed_a(cx, MotionPolicy::Full);
    click_chevron(&view, cx, 0);
    let on_grid = |v: f32| (v * SCALE).fract() == 0.0;
    for t in [16.667, 23.0, 41.0, 77.0, 133.0] {
        advance(cx, if t == 16.667 { FIRST_STEP } else { ms(t / 4.0) });
        let d = debug(&view, cx);
        for (f, y) in &d.slots {
            assert!(on_grid(*y), "{t}: slot {f} at {y}");
        }
        for c in &d.cards {
            assert!(on_grid(c.bounds.1 + c.bounds.3), "{t}: card {c:?}");
        }
        assert!(on_grid(d.reveal.unwrap().curtain), "{t}: curtain");
    }
}

#[gpui_kit::test]
fn later_frames_shape_nothing(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // c.rs is long: as a.rs collapses, its rows rise into view.
    let (view, cx) = open_cards(cx, fixture(60), 1000.);
    set_policy(cx, MotionPolicy::Full);
    let (events, _sub) = record_events(&view, cx);
    click_chevron(&view, cx, 0);
    assert!(running(&view, cx));
    let commit = all_stats(&events).len();
    for _ in 0..10 {
        advance(cx, FIRST_STEP);
    }
    assert!(!running(&view, cx));
    let stats = all_stats(&events);
    assert!(stats.len() > commit + 5);
    for s in &stats[commit..] {
        assert_eq!(s.shaped_lines, 0, "{s:?}");
    }
}

#[gpui_kit::test]
fn chevron_turns_with_its_body(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1000.);
    set_policy(cx, MotionPolicy::Full);
    assert_eq!(chevron_angle(&debug(&view, cx), 0), 0.0);
    click_chevron(&view, cx, 0);
    assert_eq!(chevron_angle(&debug(&view, cx), 0), 0.0, "commit frame");
    step_to(cx, ms(70.0));
    let angle = chevron_angle(&debug(&view, cx), 0);
    let expected = -90.0 * slide(0.5);
    assert!(
        (f64::from(angle) - expected).abs() < 1e-2,
        "70 ms: {angle}, expected {expected}"
    );
    advance(cx, ms(70.0));
    let d = debug(&view, cx);
    assert_eq!(chevron_angle(&d, 0), -90.0);
    // Settled, the closed chevron is drawn as before.
    assert_eq!(icons_named(&d, "chevron-right").len(), 1);
}

/// A flex-wrap of 50 × 10 boxes: its height depends on its width.
fn wrapping(n: usize) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .flex()
        .flex_wrap()
        .children((0..n).map(|_| div().w(px(50.)).h(px(10.))))
        .into_any_element()
}

#[gpui_kit::test]
fn hold_layout_resolves_once_at_the_target(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        fixture(10),
        card_options(LayoutMode::Auto),
        1400.,
        VIEW_H,
    );
    assert_eq!(debug(&view, cx).layout, polygloss_diff::rows::Layout::Split);
    let before = debug(&view, cx).relayouts;
    view.update(cx, |v, cx| v.hold_layout(px(1000.), cx));
    frame(cx);
    let d = debug(&view, cx);
    assert_eq!(d.layout, polygloss_diff::rows::Layout::Unified);
    assert_eq!(d.relayouts - before, 1);
    for w in [1350.0, 1300.0, 1250.0, 1200.0, 1100.0, 1000.0] {
        cx.simulate_resize(size(px(w), px(VIEW_H)));
        frame(cx);
        let d = debug(&view, cx);
        assert_eq!(d.relayouts - before, 1, "live {w}");
        assert_eq!(d.layout, polygloss_diff::rows::Layout::Unified, "live {w}");
    }
}

#[gpui_kit::test]
fn hold_layout_pins_block_and_prelude_widths(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1200.);
    view.update(cx, |v, cx| {
        v.set_prelude(Some(Rc::new(|_, _| wrapping(200))), cx);
        v.set_blocks(
            1,
            vec![BlockSpec {
                id: BlockId(5),
                anchor: BlockAnchor::Line {
                    side: Side::New,
                    line: 2,
                },
                render: Rc::new(|_, _| wrapping(150)),
            }],
            cx,
        );
    });
    settle(cx);
    let heights = |view: &gpui_kit::Entity<DiffViewport>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            let doc = v.document();
            (doc.prelude_height(), doc.blocks(1)[0].height)
        })
    };
    let wide = heights(&view, cx);
    view.update(cx, |v, cx| v.hold_layout(px(1200.), cx));
    for w in [1100.0, 1000.0, 900.0] {
        cx.simulate_resize(size(px(w), px(VIEW_H)));
        settle(cx);
        assert_eq!(heights(&view, cx), wide, "live {w}");
    }
    view.update(cx, |v, cx| v.release_layout(cx));
    settle(cx);
    let narrow = heights(&view, cx);
    assert!(
        narrow.0 > wide.0 && narrow.1 > wide.1,
        "{wide:?} → {narrow:?}"
    );
}

#[gpui_kit::test]
fn split_halves_follow_the_live_width(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        fixture(10),
        card_options(LayoutMode::Split),
        1200.,
        VIEW_H,
    );
    let theme = view.read_with(cx, |v, _| v.options().theme.clone());
    view.update(cx, |v, cx| v.hold_layout(px(1200.), cx));
    let (events, _sub) = record_events(&view, cx);
    cx.simulate_resize(size(px(900.), px(VIEW_H)));
    frame(cx);
    frame(cx);
    assert!(
        all_stats(&events).iter().all(|s| s.shaped_lines == 0),
        "nothing is shaped"
    );
    // The live card: 12 pt margins and a 1 pt border on each side.
    let (inner_x, inner_w) = (13.0, 900.0 - 26.0);
    let half = inner_x + (inner_w / 2.0f32).floor();
    // a.rs's added rows: a tint in the right half only, from the live
    // half to the card's live inner edge.
    let tints = quads_of(cx, theme.added_background);
    assert!(!tints.is_empty());
    assert!(
        tints.iter().any(|t| t.0 == half),
        "a.rs's right half: {tints:?}"
    );
    for (x, _, w, _) in tints {
        // b.rs and c.rs are one-sided: one pane across the card.
        assert!(
            x == half || x == inner_x,
            "a tint at {x}: the live half is {half}"
        );
        assert_eq!(x + w, inner_x + inner_w, "it ends at the live edge");
    }
    // Its content is clipped at the card's live edge.
    let clips: Vec<_> = shaped_quads(cx)
        .into_iter()
        .filter(|q| q.fill == Some(theme.added_background))
        .map(|q| q.clip)
        .collect();
    for (x, _, w, _) in clips {
        assert!(x + w <= inner_x + inner_w + 1e-3, "clip ends at {}", x + w);
    }
}

#[gpui_kit::test]
fn tints_reach_the_live_card_edge(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open_cards(cx, fixture(10), 1200.);
    let theme = view.read_with(cx, |v, _| v.options().theme.clone());
    view.update(cx, |v, cx| v.hold_layout(px(1200.), cx));
    cx.simulate_resize(size(px(900.), px(VIEW_H)));
    frame(cx);
    let tints = quads_of(cx, theme.added_background);
    assert!(!tints.is_empty());
    for (x, _, w, _) in tints {
        assert_eq!(
            x + w,
            900.0 - 13.0,
            "an added row's tint ends at the live inner edge"
        );
    }
}

#[gpui_kit::test]
fn release_layout_resumes_fit_width(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        fixture(10),
        card_options(LayoutMode::Auto),
        1400.,
        VIEW_H,
    );
    view.update(cx, |v, cx| v.hold_layout(px(1400.), cx));
    cx.simulate_resize(size(px(1000.), px(VIEW_H)));
    frame(cx);
    assert_eq!(debug(&view, cx).layout, polygloss_diff::rows::Layout::Split);
    view.update(cx, |v, cx| v.release_layout(cx));
    frame(cx);
    assert_eq!(
        debug(&view, cx).layout,
        polygloss_diff::rows::Layout::Unified
    );
    cx.simulate_resize(size(px(1400.), px(VIEW_H)));
    frame(cx);
    assert_eq!(debug(&view, cx).layout, polygloss_diff::rows::Layout::Split);
}
