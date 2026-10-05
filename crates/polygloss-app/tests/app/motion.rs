//! Product motion (T6.8, design §11.16, ADR-0029): `motion::enter_from`
//! samples gpui-base's executor clock, so these tests step it with
//! `advance_clock` (never the wall clock); Reduce Motion settles it on the
//! first frame and is read again on every activation of the main window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    Context, ElementId, Entity, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext, Window,
    div, point, px,
};
use polygloss_app::motion::{self, ENTER_PANEL, Entrance, ReduceMotionSource};

use crate::shell::{draw, start};
use crate::support::Sandbox;

/// A window drawing one 40 × 20 pt box that enters from 12 pt right of its
/// place, as the threads panel's content does (design §11.16).
struct Probe {
    epoch: u64,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let target = div()
            .debug_selector(|| "probe".into())
            .w(px(40.))
            .h(px(20.));
        div().size_full().child(motion::enter_from(
            "probe",
            self.epoch,
            point(px(12.), px(0.)),
            ENTER_PANEL,
            motion::ease_out_quint,
            target,
            window,
            cx,
        ))
    }
}

/// What each entrance drew last, by its id (opacity has no other window
/// into a test).
type Drawn = Rc<RefCell<HashMap<ElementId, Entrance>>>;

fn record(cx: &mut TestAppContext) -> Drawn {
    let drawn: Drawn = Rc::default();
    let sink = drawn.clone();
    cx.update(|cx| {
        motion::record_entrances(
            move |id, entrance| {
                sink.borrow_mut().insert(id.clone(), entrance);
            },
            cx,
        )
    });
    drawn
}

fn open_probe(cx: &mut TestAppContext) -> (Entity<Probe>, &mut VisualTestContext) {
    cx.add_window_view(|_, _| Probe { epoch: 1 })
}

/// Draws one frame at the clock's current time.
fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// The probe's offset from its place (its parent sits at the window's
/// origin) and its opacity, as last drawn.
fn sample(cx: &mut VisualTestContext, drawn: &Drawn) -> (Pixels, f32) {
    let bounds = cx.debug_bounds("probe").expect("the probe is painted");
    assert_eq!(bounds.origin.y, px(0.), "the probe moves sideways only");
    let opacity = drawn
        .borrow()
        .get(&ElementId::from("probe"))
        .expect("the probe entered")
        .opacity;
    (bounds.origin.x, opacity)
}

fn strictly_between(v: f32, low: f32, high: f32) -> bool {
    low < v && v < high
}

#[gpui_kit::test]
fn enter_from_starts_at_full_travel_and_settles_on_the_test_clock(cx: &mut TestAppContext) {
    let drawn = record(cx);
    let (_probe, cx) = open_probe(cx);
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(12.), 0.0), "the first frame");

    cx.executor().advance_clock(Duration::from_millis(90));
    frame(cx);
    let (x, opacity) = sample(cx, &drawn);
    assert!(strictly_between(x.as_f32(), 0., 12.), "half way: x {x:?}");
    assert!(strictly_between(opacity, 0., 1.), "half way: {opacity}");

    cx.executor().advance_clock(Duration::from_millis(180));
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(0.), 1.0), "settled");
}

#[gpui_kit::test]
fn enter_from_replays_only_for_a_new_epoch(cx: &mut TestAppContext) {
    let drawn = record(cx);
    let (probe, cx) = open_probe(cx);
    frame(cx);
    cx.executor().advance_clock(Duration::from_millis(200));
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(0.), 1.0));

    // Drawn again with the same epoch: it stays where it is.
    frame(cx);
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(0.), 1.0), "no replay");

    // A new epoch enters again from the start.
    probe.update(cx, |p, cx| {
        p.epoch = 2;
        cx.notify();
    });
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(12.), 0.0), "replayed");
    cx.executor().advance_clock(Duration::from_millis(180));
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(0.), 1.0));
}

#[gpui_kit::test]
fn enter_from_is_settled_at_once_under_reduce_motion(cx: &mut TestAppContext) {
    let drawn = record(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_probe, cx) = open_probe(cx);
    frame(cx);
    assert_eq!(sample(cx, &drawn), (px(0.), 1.0), "the first frame");
}

#[gpui_kit::test]
fn pointer_initiated_follows_the_last_input(cx: &mut TestAppContext) {
    let (_probe, cx) = open_probe(cx);
    let pointer =
        |cx: &mut VisualTestContext| cx.update(|window, _| motion::pointer_initiated(window));
    cx.simulate_keystrokes("a");
    assert!(!pointer(cx), "a key was last");
    cx.simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::none());
    assert!(pointer(cx), "the pointer was last");
    cx.simulate_keystrokes("b");
    assert!(!pointer(cx));
    cx.simulate_click(point(px(6.), px(6.)), Modifiers::none());
    assert!(pointer(cx));
}

#[gpui_kit::test]
fn reduce_motion_is_reread_on_activation(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let drawn = record(cx);
    let shell = start(cx);
    let activate = |cx: &mut VisualTestContext| {
        cx.deactivate_window();
        cx.update(|window, _| window.activate_window());
        draw(cx);
    };
    assert!(!shell.cx.update(|_, cx| cx.reduce_motion()));

    // The system turned Reduce Motion on while the app ran.
    shell
        .cx
        .update(|_, cx| cx.set_global(ReduceMotionSource(|cx| cx.set_reduce_motion(true))));
    assert!(!shell.cx.update(|_, cx| cx.reduce_motion()), "not before");
    activate(shell.cx);
    assert!(
        shell.cx.update(|_, cx| cx.reduce_motion()),
        "read on activation"
    );
    let (probe, probe_window) = {
        let (probe, pcx) = shell.cx.add_window_view(|_, _| Probe { epoch: 1 });
        frame(pcx);
        assert_eq!(
            sample(pcx, &drawn),
            (px(0.), 1.0),
            "settled on its first frame"
        );
        (probe, pcx.update(|window, _| window.window_handle()))
    };

    // And off again.
    shell
        .cx
        .update(|_, cx| cx.set_global(ReduceMotionSource(|cx| cx.set_reduce_motion(false))));
    activate(shell.cx);
    assert!(!shell.cx.update(|_, cx| cx.reduce_motion()));
    let pcx = VisualTestContext::from_window(probe_window, &shell.cx.cx).into_mut();
    probe.update(pcx, |p, cx| {
        p.epoch = 2;
        cx.notify();
    });
    frame(pcx);
    assert_eq!(sample(pcx, &drawn), (px(12.), 0.0), "it moves again");
}

/// The threads panel's content: its left edge's offset from the pane's, and
/// the opacity its entrance drew on the last frame (`None`: no entrance was
/// drawn since `drawn` was last cleared).
fn panel_sample(cx: &mut VisualTestContext, drawn: &Drawn) -> (f32, Option<f32>) {
    let pane = crate::shell::bounds(cx, "threads-pane");
    let content = crate::shell::bounds(cx, "threads-panel");
    let opacity = drawn
        .borrow()
        .get(&ElementId::from("threads-panel"))
        .map(|e| e.opacity);
    ((content.left() - pane.left()).as_f32(), opacity)
}

/// Clears what entrances drew, then draws one frame.
fn fresh_frame(cx: &mut VisualTestContext, drawn: &Drawn) {
    drawn.borrow_mut().clear();
    frame(cx);
}

/// Opens `code_change_repo`'s compare review in a started app.
fn review(
    shell: &mut crate::shell::Shell,
    repo: &crate::support::FixtureRepo,
) -> Entity<polygloss_app::review_tab::ReviewTab> {
    let tab = shell
        .open(crate::shell::compare_req(repo.path()))
        .expect("open the review");
    assert!(!tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    tab
}

/// A click on the toolbar's threads button.
fn click_threads_button(cx: &mut VisualTestContext) {
    let at = crate::shell::bounds(cx, "toggle-threads-panel").center();
    cx.simulate_click(at, Modifiers::none());
}

#[gpui_kit::test]
fn pointer_open_slides_the_panel_content_in(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let drawn = record(cx);
    let repo = crate::support::code_change_repo();
    let mut shell = start(cx);
    let tab = review(&mut shell, &repo);

    click_threads_button(shell.cx);
    fresh_frame(shell.cx, &drawn);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let (start_x, start_opacity) = panel_sample(shell.cx, &drawn);
    shell.cx.executor().advance_clock(Duration::from_millis(90));
    fresh_frame(shell.cx, &drawn);
    let (mid_x, mid_opacity) = panel_sample(shell.cx, &drawn);
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(180));
    fresh_frame(shell.cx, &drawn);
    let (end_x, end_opacity) = panel_sample(shell.cx, &drawn);
    assert_eq!(start_x - end_x, 12.0, "the first frame: 12 pt right");
    assert_eq!(start_opacity, Some(0.0));
    assert!(
        strictly_between(mid_x - end_x, 0., 12.),
        "half way: {mid_x}"
    );
    let mid_opacity = mid_opacity.expect("still entering");
    assert!(strictly_between(mid_opacity, 0., 1.), "{mid_opacity}");
    assert_eq!(end_opacity.unwrap_or(1.0), 1.0, "settled");

    // Settled, it stays: away to Home and back plays nothing again.
    shell.cx.simulate_keystrokes("cmd-0");
    draw(shell.cx);
    assert!(crate::shell::painted(shell.cx, "threads-pane").is_none());
    shell.cx.simulate_keystrokes("cmd-1");
    draw(shell.cx);
    fresh_frame(shell.cx, &drawn);
    assert_eq!(panel_sample(shell.cx, &drawn), (end_x, None), "no replay");

    // Closing is instant; opening again enters again.
    click_threads_button(shell.cx);
    fresh_frame(shell.cx, &drawn);
    assert!(crate::shell::painted(shell.cx, "threads-pane").is_none());
    click_threads_button(shell.cx);
    fresh_frame(shell.cx, &drawn);
    assert_eq!(panel_sample(shell.cx, &drawn), (end_x + 12.0, Some(0.0)));
}

#[gpui_kit::test]
fn keyboard_toggle_and_restore_show_the_panel_at_once(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let drawn = record(cx);
    let repo = crate::support::code_change_repo();
    let mut shell = start(cx);
    let tab = review(&mut shell, &repo);

    // The palette's or View menu's toggle, from the keyboard.
    shell.cx.simulate_keystrokes("escape");
    shell
        .cx
        .dispatch_action(polygloss_app::review_tab::panes::ToggleThreadsPanel);
    fresh_frame(shell.cx, &drawn);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let (at_once, entrance) = panel_sample(shell.cx, &drawn);
    assert_eq!(entrance, None, "no entrance");
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(500));
    fresh_frame(shell.cx, &drawn);
    assert_eq!(
        panel_sample(shell.cx, &drawn),
        (at_once, None),
        "at its end"
    );

    // Restored with the review (closed and opened again): at once too.
    shell.cx.simulate_keystrokes("cmd-w");
    drop(tab);
    draw(shell.cx);
    let tab = shell
        .open(crate::shell::compare_req(repo.path()))
        .expect("reopen");
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    fresh_frame(shell.cx, &drawn);
    assert_eq!(panel_sample(shell.cx, &drawn), (at_once, None));
}

#[gpui_kit::test]
fn reduce_motion_shows_the_panel_at_once(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let drawn = record(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let repo = crate::support::code_change_repo();
    let mut shell = start(cx);
    let tab = review(&mut shell, &repo);

    click_threads_button(shell.cx);
    fresh_frame(shell.cx, &drawn);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let (first, opacity) = panel_sample(shell.cx, &drawn);
    assert_eq!(opacity.unwrap_or(1.0), 1.0, "opaque on its first frame");
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(500));
    fresh_frame(shell.cx, &drawn);
    assert_eq!(panel_sample(shell.cx, &drawn).0, first, "never moved");
}
