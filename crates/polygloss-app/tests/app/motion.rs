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
