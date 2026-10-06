//! The motion core in the app (T7.2, design §11.16, ADR-0030): the policy
//! override, `motion::sample` on the executor clock, settling (keys before
//! their bindings, mouse downs by freezing, scrolls and resizes), the
//! `slide` and `clip` wrappers, `Exit`, the kit's tokens, and the two M6
//! entrances (`enter_from`, retired by T7.9 and T7.10).
//!
//! Tests step the clock by ADR-0030's stepping protocol
//! (`support::motion::step_to`); expected values are this file's own: a
//! bisection solver for the Béziers and hand-written durations.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, ElementId, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, KeyBinding, Modifiers, MouseButton, ParentElement as _,
    Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, TestAppContext, TouchPhase, VisualTestContext, Window, bounds,
    div, point, px, rgb, size,
};
use polygloss_app::motion::exit::{Exit, ExitFrame, ExitHitbox, ExitSpec, Placement};
use polygloss_app::motion::{
    self, ENTER_PANEL, Initiator, Motion, MotionPolicy, Recorded, ReduceMotionSource, Reduced,
    ReducedPlay, Sample, Settle, Track, settle, tokens, wrap,
};

use crate::shell::{draw, start};
use crate::support::Sandbox;
use crate::support::motion::{advance, frame, requested_frames, step_to};

fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

fn strictly_between(v: f32, low: f32, high: f32) -> bool {
    low < v && v < high
}

/// `cubic-bezier(x1, y1, x2, y2)` at `t` by bisection on x.
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

/// SLIDE.
fn slide(t: f64) -> f64 {
    bezier(0.25, 1.0, 0.5, 1.0, t)
}

/// OUT.
fn out(t: f64) -> f64 {
    bezier(0.16, 1.0, 0.3, 1.0, t)
}

/// `round(v · scale) / scale` at the test window's scale, 2.
fn quantized(v: f64) -> f32 {
    ((v * 2.0).round() / 2.0) as f32
}

// ---------------------------------------------------------------------------
// The probe: a button that moves with track A (x 100 → 300 as A goes 0 → 1)
// over a frame as wide as 200 · A, and a second track B, in a host view that
// paints the settle root first, as `MainWindow` does.

gpui_kit::actions!(motion_probe, [Toggle, Jump, Reverse, StartB]);

/// A's motion: 240 ms in, 180 ms out on SLIDE, by pointer and keyboard.
const A: Motion = Motion {
    enter: Duration::from_millis(240),
    exit: Duration::from_millis(180),
    easing: tokens::slide,
    animates: &[Initiator::Pointer, Initiator::Keyboard],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Fade,
    },
};

/// B's: 150 ms on SLIDE, by pointer.
const B: Motion = Motion {
    enter: Duration::from_millis(150),
    exit: Duration::from_millis(110),
    easing: tokens::slide,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Fade,
    },
};

/// What a click on the probe's button does.
#[derive(Clone, Copy, PartialEq)]
enum OnClick {
    Count,
    /// Reverses A, from where it is.
    Reverse,
    /// The same through `window.dispatch_action`, which gpui defers.
    ReverseByAction,
    /// Settles A and starts B.
    StartB,
    StartBByAction,
}

struct Probe {
    focus: FocusHandle,
    a: Track,
    b: Track,
    open: bool,
    on_click: OnClick,
    clicks: usize,
    /// What the last frame drew of A and B.
    drawn: (Sample, Sample),
    /// Whether A was settled when `j`'s action ran.
    settled_at_jump: Option<bool>,
}

impl Probe {
    fn new(cx: &mut Context<Self>) -> Self {
        let settled = Track::new(0.0, 1.0).sample(cx.background_executor().now());
        Probe {
            focus: cx.focus_handle(),
            a: Track::new(0.0, 1.0),
            b: Track::new(0.0, 1.0),
            open: false,
            on_click: OnClick::Count,
            clicks: 0,
            drawn: (settled, settled),
            settled_at_jump: None,
        }
    }

    /// Opens or closes A.
    fn toggle(&mut self, initiator: Initiator, cx: &mut Context<Self>) {
        self.open = !self.open;
        let to = if self.open { 1.0 } else { 0.0 };
        let now = cx.background_executor().now();
        self.a.retarget(to, &A, initiator, motion::policy(cx), now);
        cx.notify();
    }

    fn start_b(&mut self, cx: &mut Context<Self>) {
        self.a.settle();
        let now = cx.background_executor().now();
        self.b
            .retarget(1.0, &B, Initiator::Pointer, motion::policy(cx), now);
        cx.notify();
    }

    fn click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clicks += 1;
        match self.on_click {
            OnClick::Count => {}
            OnClick::Reverse => self.toggle(Initiator::Pointer, cx),
            OnClick::ReverseByAction => window.dispatch_action(Box::new(Reverse), cx),
            OnClick::StartB => self.start_b(cx),
            OnClick::StartBByAction => window.dispatch_action(Box::new(StartB), cx),
        }
    }
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let a = motion::sample(&mut self.a, window, cx);
        let b = motion::sample(&mut self.b, window, cx);
        self.drawn = (a, b);
        if !(a.settled && b.settled) {
            let (frozen, settled) = (cx.entity().downgrade(), cx.entity().downgrade());
            settle::register(
                Some(Box::new(Toggle)),
                move |now, cx| {
                    frozen
                        .update(cx, |p, cx| {
                            p.a.freeze(now);
                            p.b.freeze(now);
                            cx.notify();
                        })
                        .ok();
                },
                move |why, cx| {
                    settled
                        .update(cx, |p, cx| {
                            for track in [&mut p.a, &mut p.b] {
                                if why == Settle::All || track.is_frozen() {
                                    track.settle();
                                }
                            }
                            cx.notify();
                        })
                        .ok();
                },
                window,
                cx,
            );
        }
        let scale = window.scale_factor();
        let x = motion::quantize(100.0 + 200.0 * a.value, scale);
        let width = motion::quantize(200.0 * a.value, scale);
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Probe")
            .on_action(
                cx.listener(|p, _: &Toggle, window, cx| p.toggle(motion::initiator(window), cx)),
            )
            .on_action(cx.listener(|p, _: &Jump, _, _| p.settled_at_jump = Some(p.a.is_settled())))
            .on_action(cx.listener(|p, _: &Reverse, _, cx| p.toggle(Initiator::Pointer, cx)))
            .on_action(cx.listener(|p, _: &StartB, _, cx| p.start_b(cx)))
            .child(
                div()
                    .debug_selector(|| "probe-frame".into())
                    .absolute()
                    .left_0()
                    .top_0()
                    .h(px(40.))
                    .w(px(width)),
            )
            .child(
                div()
                    .id("probe-button")
                    .debug_selector(|| "probe-button".into())
                    .absolute()
                    .left(px(x))
                    .top(px(100.))
                    .w(px(40.))
                    .h(px(20.))
                    .on_click(cx.listener(|p, _, window, cx| p.click(window, cx))),
            )
    }
}

/// The probe's window root: the settle root first, then the probe, cached
/// or not.
struct Host {
    probe: Entity<Probe>,
    cached: bool,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let probe = if self.cached {
            self.probe
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element()
        } else {
            self.probe.clone().into_any_element()
        };
        div().size_full().child(settle::root()).child(probe)
    }
}

fn open_probe(
    cx: &mut TestAppContext,
    cached: bool,
) -> (Entity<Host>, Entity<Probe>, &mut VisualTestContext) {
    cx.update(|cx| {
        motion::init(cx);
        cx.bind_keys([
            KeyBinding::new("t", Toggle, Some("Probe")),
            KeyBinding::new("j", Jump, Some("Probe")),
        ]);
    });
    let (host, cx) = cx.add_window_view(|window, cx| {
        cx.observe_window_bounds(window, settle::bounds_observer(window))
            .detach();
        Host {
            probe: cx.new(Probe::new),
            cached,
        }
    });
    let probe = host.read_with(cx, |h, _| h.probe.clone());
    cx.update(|window, cx| {
        let focus = probe.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    frame(cx);
    (host, probe, cx)
}

/// Opens or closes A by `initiator` and draws the commit frame.
fn toggle(cx: &mut VisualTestContext, probe: &Entity<Probe>, initiator: Initiator) {
    probe.update(cx, |p, cx| p.toggle(initiator, cx));
    frame(cx);
}

fn drawn(cx: &mut VisualTestContext, probe: &Entity<Probe>) -> (Sample, Sample) {
    probe.read_with(cx, |p, _| p.drawn)
}

fn button(cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.debug_bounds("probe-button")
        .expect("the button is painted")
}

fn set_policy(cx: &mut VisualTestContext, policy: Option<MotionPolicy>) {
    cx.update(|_, cx| motion::set_override(policy, cx));
}

#[gpui_kit::test]
fn reduced_fade_has_no_travel(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    set_policy(cx, Some(MotionPolicy::Reduced));
    toggle(cx, &probe, Initiator::Pointer);
    let at_end = |cx: &mut VisualTestContext| {
        assert_eq!(button(cx).origin.x, px(300.), "no travel");
        drawn(cx, &probe).0
    };
    assert_eq!(at_end(cx).opacity, 0.0, "the commit frame");
    step_to(cx, tokens::FIRST_STEP);
    assert!(strictly_between(at_end(cx).opacity, 0.0, 1.0));
    advance(cx, ms(75.0) - tokens::FIRST_STEP);
    let half = at_end(cx);
    assert!(
        strictly_between(half.opacity, 0.0, 1.0),
        "at half of QUICK: {half:?}"
    );
    advance(cx, ms(80.0));
    let end = at_end(cx);
    assert_eq!((end.opacity, end.settled), (1.0, true));
}

#[gpui_kit::test]
fn reduced_close_holds_then_snaps(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Programmatic);
    let width = |cx: &mut VisualTestContext| {
        cx.debug_bounds("probe-frame")
            .expect("the frame is painted")
            .size
            .width
    };
    assert_eq!(width(cx), px(200.), "open");
    set_policy(cx, Some(MotionPolicy::Reduced));
    toggle(cx, &probe, Initiator::Pointer);
    assert_eq!(width(cx), px(200.), "the commit frame holds the frame");
    step_to(cx, ms(50.0));
    assert_eq!(width(cx), px(200.), "unchanged while it fades");
    let half = drawn(cx, &probe).0;
    assert!(strictly_between(half.opacity, 0.0, 1.0), "{half:?}");
    advance(cx, ms(66.667));
    assert_eq!(width(cx), px(0.), "snapped after MICRO");
    assert!(drawn(cx, &probe).0.settled);
}

#[gpui_kit::test]
fn off_settles_on_the_first_frame_and_requests_no_frame(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    set_policy(cx, Some(MotionPolicy::Off));
    requested_frames(cx);
    toggle(cx, &probe, Initiator::Pointer);
    let commit = drawn(cx, &probe).0;
    assert_eq!((commit.value, commit.settled), (1.0, true));
    assert_eq!(button(cx).origin.x, px(300.));
    assert_eq!(requested_frames(cx), 0, "no frame after the commit");

    // Full, the same open requests the next frame.
    set_policy(cx, Some(MotionPolicy::Full));
    toggle(cx, &probe, Initiator::Pointer);
    assert!(!drawn(cx, &probe).0.settled);
    assert!(requested_frames(cx) > 0);
}

#[gpui_kit::test]
fn any_override_settles_kit_motion(cx: &mut TestAppContext) {
    for policy in [MotionPolicy::Off, MotionPolicy::Full, MotionPolicy::Reduced] {
        cx.update(|cx| {
            assert!(!cx.reduce_motion());
            motion::set_override(Some(policy), cx);
            assert!(cx.reduce_motion(), "{policy:?} settles kit motion");
            assert_eq!(motion::policy(cx), policy, "and is the app's policy");
            motion::set_override(None, cx);
        });
    }
}

#[gpui_kit::test]
fn clearing_the_override_restores_the_flag(cx: &mut TestAppContext) {
    cx.update(|cx| {
        motion::set_override(Some(MotionPolicy::Off), cx);
        // A second override keeps the flag the first one found.
        motion::set_override(Some(MotionPolicy::Full), cx);
        motion::set_override(None, cx);
        assert!(!cx.reduce_motion(), "restored");
        assert_eq!(motion::policy(cx), MotionPolicy::Full, "the flag decides");

        cx.set_reduce_motion(true);
        motion::set_override(Some(MotionPolicy::Off), cx);
        motion::set_override(None, cx);
        assert!(cx.reduce_motion(), "restored");
        assert_eq!(motion::policy(cx), MotionPolicy::Reduced);
    });
}

// ---------------------------------------------------------------------------
// The wrappers.

#[derive(Clone, Copy)]
enum Wrap {
    None,
    Slide(Point<Pixels>),
    Clip(Bounds<Pixels>),
}

fn wrapped(how: Wrap, child: impl IntoElement) -> AnyElement {
    match how {
        Wrap::None => child.into_any_element(),
        Wrap::Slide(offset) => wrap::slide(offset, child),
        Wrap::Clip(mask) => wrap::clip(mask, child),
    }
}

/// A 50 pt sibling, then a 100 × 40 container holding a `size_full`
/// clickable child, wrapped or not.
struct Wrapped {
    how: Wrap,
    clicks: usize,
}

impl Render for Wrapped {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let child = div()
            .id("child")
            .debug_selector(|| "child".into())
            .size_full()
            .on_click(cx.listener(|w, _, _, _| w.clicks += 1));
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .debug_selector(|| "sibling".into())
                    .w(px(50.))
                    .h(px(40.)),
            )
            .child(
                div()
                    .debug_selector(|| "container".into())
                    .relative()
                    .w(px(100.))
                    .h(px(40.))
                    .child(wrapped(self.how, child)),
            )
    }
}

#[gpui_kit::test]
fn slide_moves_paint_not_layout(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Wrapped {
        how: Wrap::None,
        clicks: 0,
    });
    frame(cx);
    let selector =
        |cx: &mut VisualTestContext, name: &'static str| cx.debug_bounds(name).expect("painted");
    let (sibling, child) = (selector(cx, "sibling"), selector(cx, "child"));
    assert_eq!(
        child.size,
        size(px(100.), px(40.)),
        "size_full fills its container"
    );

    view.update(cx, |w, cx| {
        w.how = Wrap::Slide(point(px(30.), px(10.)));
        cx.notify();
    });
    frame(cx);
    assert_eq!(
        selector(cx, "sibling"),
        sibling,
        "the sibling's layout is unchanged"
    );
    let slid = selector(cx, "child");
    assert_eq!(slid.size, child.size, "the child's layout is unchanged");
    assert_eq!(
        slid.origin,
        child.origin + point(px(30.), px(10.)),
        "painted 30, 10 over"
    );

    // The hitbox moved with the paint.
    let container = selector(cx, "container");
    cx.simulate_click(container.origin + point(px(10.), px(5.)), Modifiers::none());
    assert_eq!(
        view.read_with(cx, |w, _| w.clicks),
        0,
        "where it was laid out"
    );
    cx.simulate_click(
        container.origin + point(px(115.), px(20.)),
        Modifiers::none(),
    );
    assert_eq!(
        view.read_with(cx, |w, _| w.clicks),
        1,
        "where it was painted"
    );
}

/// A 100 pt scroller over 400 pt of content, wrapped or not.
struct Scroller {
    how: Wrap,
}

impl Render for Scroller {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let scroller = div()
            .id("scroller")
            .debug_selector(|| "scroller".into())
            .overflow_y_scroll()
            .w(px(200.))
            .h(px(100.))
            .child(
                div()
                    .debug_selector(|| "scroll-content".into())
                    .w(px(200.))
                    .h(px(400.)),
            );
        div().size_full().child(wrapped(self.how, scroller))
    }
}

#[gpui_kit::test]
fn wrappers_keep_the_childs_element_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Scroller { how: Wrap::None });
    frame(cx);
    let scrolled = |cx: &mut VisualTestContext| {
        let scroller = cx.debug_bounds("scroller").expect("painted");
        let content = cx.debug_bounds("scroll-content").expect("painted");
        (content.top() - scroller.top()).as_f32()
    };
    let at = cx.debug_bounds("scroller").expect("painted").center();
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    frame(cx);
    assert_eq!(scrolled(cx), -40.0, "scrolled 40 pt");
    for how in [
        Wrap::Slide(point(px(12.), px(0.))),
        Wrap::Clip(bounds(point(px(0.), px(0.)), size(px(150.), px(80.)))),
        Wrap::None,
    ] {
        view.update(cx, |s, cx| {
            s.how = how;
            cx.notify();
        });
        for _ in 0..3 {
            frame(cx);
            assert_eq!(
                scrolled(cx),
                -40.0,
                "the scroll offset survives the wrapper"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Settling (ADR-0030 rule 4).

#[gpui_kit::test]
fn pointer_movement_never_settles(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    cx.simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::none());
    advance(cx, ms(60.0));
    cx.simulate_mouse_move(point(px(400.), px(300.)), None, Modifiers::none());
    frame(cx);
    let half = drawn(cx, &probe).0;
    assert!(
        !half.settled && strictly_between(half.value, 0.0, 1.0),
        "{half:?}"
    );
}

#[gpui_kit::test]
fn bound_keys_settle_before_their_action_runs(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(120.0));
    assert!(!drawn(cx, &probe).0.settled);
    // `j` is bound to an action that stops propagation: a key listener
    // would never see it.
    cx.simulate_keystrokes("j");
    assert_eq!(
        probe.read_with(cx, |p, _| p.settled_at_jump),
        Some(true),
        "settled before the action ran"
    );
}

#[gpui_kit::test]
fn a_lone_modifier_never_settles(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    cx.simulate_modifiers_change(Modifiers::shift());
    cx.simulate_modifiers_change(Modifiers::none());
    advance(cx, ms(60.0));
    let half = drawn(cx, &probe).0;
    assert!(
        !half.settled && strictly_between(half.value, 0.0, 1.0),
        "{half:?}"
    );
}

#[gpui_kit::test]
fn own_shortcut_retargets(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(120.0));
    let at = drawn(cx, &probe).0.value;
    assert!(strictly_between(at, 0.0, 1.0));
    // `t` is bound to the probe's registered action: its handler reverses
    // A from where it is.
    cx.simulate_keystrokes("t");
    let reversed = drawn(cx, &probe).0;
    assert_eq!(
        (reversed.value, reversed.settled),
        (at, false),
        "from the sampled value"
    );
    advance(cx, tokens::FIRST_STEP);
    let next = drawn(cx, &probe).0.value;
    assert!(strictly_between(next, 0.0, at), "closing: {next}");
}

#[gpui_kit::test]
fn scroll_resize_and_fullscreen_settle(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    let running = |cx: &mut VisualTestContext| {
        toggle(cx, &probe, Initiator::Pointer);
        step_to(cx, ms(60.0));
        assert!(!drawn(cx, &probe).0.settled);
    };
    let settled_at_its_end = |cx: &mut VisualTestContext, what: &str| {
        frame(cx);
        let open = probe.read_with(cx, |p, _| p.open);
        let end = drawn(cx, &probe).0;
        assert!(end.settled, "{what}");
        assert_eq!(end.value, if open { 1.0 } else { 0.0 }, "{what}");
    };

    running(cx);
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(5.), px(5.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    settled_at_its_end(cx, "a scroll");

    running(cx);
    cx.simulate_resize(size(px(900.), px(700.)));
    settled_at_its_end(cx, "a resize");

    // Entering fullscreen changes the content size (a resize, above); a
    // move to a display of another scale changes the scale factor.
    running(cx);
    cx.simulate_scale_factor_change(1.0);
    settled_at_its_end(cx, "a scale change");
}

#[gpui_kit::test]
fn a_window_move_never_settles(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    // gpui reports a move as a bounds change with the content size
    // unchanged.
    cx.update(|window, cx| window.bounds_changed(cx));
    advance(cx, ms(60.0));
    let half = drawn(cx, &probe).0;
    assert!(
        !half.settled && strictly_between(half.value, 0.0, 1.0),
        "{half:?}"
    );
}

#[gpui_kit::test]
fn mouse_down_freezes_until_mouse_up(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    let (pressed_at, held) = (button(cx), drawn(cx, &probe).0);
    assert!(strictly_between(held.value, 0.0, 1.0));
    cx.simulate_mouse_down(pressed_at.center(), MouseButton::Left, Modifiers::none());
    // Three frames (50 ms) of a long press.
    for _ in 0..3 {
        advance(cx, tokens::FIRST_STEP);
        assert_eq!(drawn(cx, &probe).0, held, "the track holds");
        assert_eq!(button(cx), pressed_at, "the button holds");
    }
    cx.simulate_mouse_up(pressed_at.center(), MouseButton::Left, Modifiers::none());
    assert_eq!(
        probe.read_with(cx, |p, _| p.clicks),
        1,
        "the click hit the button"
    );
    frame(cx);
    let end = drawn(cx, &probe).0;
    assert_eq!(
        (end.value, end.settled),
        (1.0, true),
        "settled after the mouse up"
    );
}

#[gpui_kit::test]
fn a_frozen_track_requests_no_frames(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    cx.simulate_mouse_down(
        point(px(5.), px(300.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    // The frame the last running frame asked for, before the press.
    requested_frames(cx);
    for _ in 0..3 {
        cx.executor().advance_clock(tokens::FIRST_STEP);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(requested_frames(cx), 0, "no frames while frozen");
    }
    cx.simulate_mouse_up(
        point(px(5.), px(300.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert!(probe.read_with(cx, |p, _| p.a.is_settled()));
}

#[gpui_kit::test]
fn a_cached_owner_settles_at_the_mouse_up(cx: &mut TestAppContext) {
    let (host, probe, cx) = open_probe(cx, true);
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    cx.simulate_mouse_down(
        point(px(5.), px(300.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert!(probe.read_with(cx, |p, _| p.a.is_frozen()));
    // Frames where only the host renders: the cached probe is reused, so
    // it registers nothing in them.
    for _ in 0..2 {
        host.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    cx.simulate_mouse_up(
        point(px(5.), px(300.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert!(
        probe.read_with(cx, |p, _| p.a.is_settled()),
        "the mouse down's set reached it"
    );
}

#[gpui_kit::test]
fn the_triggers_own_click_retargets(cx: &mut TestAppContext) {
    for on_click in [OnClick::Reverse, OnClick::ReverseByAction] {
        let (_host, probe, cx) = open_probe(cx, false);
        probe.update(cx, |p, _| p.on_click = on_click);
        toggle(cx, &probe, Initiator::Pointer);
        step_to(cx, ms(60.0));
        let (pressed_at, held) = (button(cx), drawn(cx, &probe).0.value);
        cx.simulate_mouse_down(pressed_at.center(), MouseButton::Left, Modifiers::none());
        advance(cx, tokens::FIRST_STEP);
        assert_eq!(button(cx), pressed_at, "frozen under the pointer");
        cx.simulate_mouse_up(pressed_at.center(), MouseButton::Left, Modifiers::none());
        frame(cx);
        let reversed = drawn(cx, &probe).0;
        assert_eq!(
            (reversed.value, reversed.settled),
            (held, false),
            "{}: reversed from the frozen value",
            on_click as u8
        );
        // The close lasts 180 ms × the share of the open travelled.
        let close = 180.0 * slide(60.0 / 240.0);
        step_to(cx, ms(close / 2.0));
        let half = drawn(cx, &probe).0.value;
        assert!(strictly_between(half, 0.0, held), "half way back: {half}");
    }
}

#[gpui_kit::test]
fn a_motion_the_click_starts_runs_on(cx: &mut TestAppContext) {
    for on_click in [OnClick::StartB, OnClick::StartBByAction] {
        let (_host, probe, cx) = open_probe(cx, false);
        probe.update(cx, |p, _| p.on_click = on_click);
        toggle(cx, &probe, Initiator::Pointer);
        step_to(cx, ms(60.0));
        let at = button(cx).center();
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        frame(cx);
        let (a, b) = drawn(cx, &probe);
        assert_eq!((a.value, a.settled), (1.0, true), "A at its end");
        assert!(!b.settled, "B started");
        step_to(cx, ms(75.0));
        let b = drawn(cx, &probe).1;
        assert!(
            !b.settled && strictly_between(b.value, 0.0, 1.0),
            "B runs on at half its duration: {b:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Exits.

/// An exit's copy: a 40 × 20 red box with no listeners.
fn copy(_: &u8, _: &mut Window, _: &mut App) -> AnyElement {
    div()
        .debug_selector(|| "exit-copy".into())
        .w(px(40.))
        .h(px(20.))
        .bg(rgb(0xff0000))
        .into_any_element()
}

/// A notice-like exit: opacity over MICRO on OUT.
const FADE: Motion = Motion {
    enter: tokens::QUICK,
    exit: tokens::MICRO,
    easing: tokens::out,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Fade,
    },
};

/// A Home row's height: 150 ms on SLIDE.
const CLOSE: Motion = Motion {
    enter: tokens::BASE,
    exit: Duration::from_millis(150),
    easing: tokens::slide,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Snap,
        exit: ReducedPlay::Snap,
    },
};

/// An overlay exit over a clickable "beneath" box at 100, 100.
struct Overlay {
    exit: Option<Exit<u8>>,
    last: Option<ExitFrame>,
    beneath: usize,
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last = self.exit.as_mut().and_then(|e| e.frame(window, cx));
        let copy = self.exit.as_mut().and_then(|e| e.render(window, cx));
        if copy.is_none() {
            self.exit = None;
        }
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("beneath")
                    .absolute()
                    .left(px(100.))
                    .top(px(100.))
                    .w(px(40.))
                    .h(px(20.))
                    .on_click(cx.listener(|o, _, _, _| o.beneath += 1)),
            )
            .children(copy)
    }
}

fn start_overlay<'a>(
    cx: &'a mut TestAppContext,
    hitbox: ExitHitbox,
    height: Option<&'static Motion>,
) -> (Entity<Overlay>, &'a mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, _| Overlay {
        exit: None,
        last: None,
        beneath: 0,
    });
    frame(cx);
    cx.update(|window, cx| {
        view.update(cx, |o, cx| {
            let spec = ExitSpec {
                placement: Placement::Overlay(bounds(
                    point(px(100.), px(100.)),
                    size(px(40.), px(20.)),
                )),
                hitbox,
                motion: &FADE,
                travel: point(px(0.), px(-tokens::NUDGE)),
                height,
            };
            o.exit = Exit::start(0, copy, spec, Initiator::Pointer, window, cx);
            assert!(o.exit.is_some(), "a pointer exit plays in Full");
            cx.notify();
        })
    });
    frame(cx);
    (view, cx)
}

fn copy_top(cx: &mut VisualTestContext) -> f32 {
    cx.debug_bounds("exit-copy")
        .expect("the copy is painted")
        .top()
        .as_f32()
}

#[gpui_kit::test]
fn exit_lives_until_its_longest_channel_settles(cx: &mut TestAppContext) {
    let (view, cx) = start_overlay(cx, ExitHitbox::None, Some(&CLOSE));
    step_to(cx, ms(120.0));
    let at = view
        .read_with(cx, |o, _| o.last)
        .expect("alive at 120 ms: its height runs 150 ms");
    assert_eq!(at.opacity, 0.0, "the 100 ms opacity has settled");
    assert!(strictly_between(at.height, 0.0, 1.0), "{at:?}");
    assert!(cx.debug_bounds("exit-copy").is_some(), "rendered");
    advance(cx, ms(46.667));
    assert!(view.read_with(cx, |o, _| o.last.is_none() && o.exit.is_none()));
    assert!(cx.debug_bounds("exit-copy").is_none(), "gone after 150 ms");
}

#[gpui_kit::test]
fn overlay_exit_is_inert(cx: &mut TestAppContext) {
    for (hitbox, beneath) in [(ExitHitbox::None, 1), (ExitHitbox::Occlude, 0)] {
        let (view, cx) = start_overlay(cx, hitbox, None);
        assert_eq!(copy_top(cx), 100.0, "the commit frame: in place");
        step_to(cx, tokens::FIRST_STEP);
        assert_eq!(copy_top(cx), 100.0 + quantized(-4.0 * out(16.667 / 100.0)));
        advance(cx, ms(50.0) - tokens::FIRST_STEP);
        assert_eq!(copy_top(cx), 100.0 + quantized(-4.0 * out(0.5)), "50 ms");
        // Inside both the copy and the box beneath it.
        cx.simulate_click(point(px(120.), px(108.)), Modifiers::none());
        assert_eq!(view.read_with(cx, |o, _| o.beneath), beneath);
    }
}

/// A list of 60 pt rows 12 apart; a removed row leaves as an in-flow exit.
struct List {
    rows: Vec<u8>,
    exit: Option<(usize, Exit<u8>)>,
    last: Option<ExitFrame>,
    slot_had_id: Option<bool>,
}

fn row_copy(row: &u8, _: &mut Window, _: &mut App) -> AnyElement {
    let row = *row;
    div()
        .debug_selector(move || format!("copy-{row}"))
        .w(px(200.))
        .h(px(60.))
        .into_any_element()
}

impl Render for List {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut children: Vec<AnyElement> = self
            .rows
            .iter()
            .map(|&row| {
                div()
                    .debug_selector(move || format!("row-{row}"))
                    .w(px(200.))
                    .h(px(60.))
                    .into_any_element()
            })
            .collect();
        self.last = None;
        if let Some((ix, exit)) = self.exit.as_mut() {
            self.last = exit.frame(window, cx);
            match exit.render(window, cx) {
                Some(slot) => {
                    self.slot_had_id = Some(gpui_kit::Element::id(&slot).is_some());
                    children.insert(*ix, slot.w(px(200.)).into_any_element());
                }
                None => self.exit = None,
            }
        }
        div().size_full().child(
            div()
                .debug_selector(|| "list".into())
                .flex()
                .flex_col()
                .gap(px(12.))
                .w(px(200.))
                .children(children),
        )
    }
}

fn open_list(cx: &mut TestAppContext) -> (Entity<List>, &mut VisualTestContext) {
    let (list, cx) = cx.add_window_view(|_, _| List {
        rows: vec![0, 1, 2],
        exit: None,
        last: None,
        slot_had_id: None,
    });
    frame(cx);
    assert_eq!(top(cx, "row-1"), 72.0);
    assert_eq!(top(cx, "row-2"), 144.0);
    // Row 1 removed: its copy leaves in its slot.
    cx.update(|window, cx| {
        list.update(cx, |l, cx| {
            l.rows.remove(1);
            let spec = ExitSpec {
                placement: Placement::InFlow {
                    height: px(60.),
                    gap: px(12.),
                },
                hitbox: ExitHitbox::Occlude,
                motion: &FADE,
                travel: point(px(0.), px(0.)),
                height: Some(&CLOSE),
            };
            let exit = Exit::start(1, row_copy, spec, Initiator::Pointer, window, cx);
            l.exit = Some((1, exit.expect("it plays")));
            cx.notify();
        })
    });
    frame(cx);
    (list, cx)
}

fn top(cx: &mut VisualTestContext, name: &'static str) -> f32 {
    crate::shell::bounds(cx, name).top().as_f32()
}

fn list_height(cx: &mut VisualTestContext) -> f32 {
    crate::shell::bounds(cx, "list").size.height.as_f32()
}

#[gpui_kit::test]
fn in_flow_exit_keeps_its_slot(cx: &mut TestAppContext) {
    let (list, cx) = open_list(cx);
    assert_eq!(
        top(cx, "copy-1"),
        72.0,
        "the commit frame: the copy where the row was"
    );
    assert_eq!(top(cx, "row-2"), 144.0, "and the row below where it was");
    assert_eq!(
        list.read_with(cx, |l, _| l.slot_had_id),
        Some(false),
        "no element id"
    );
    step_to(cx, ms(75.0));
    assert!(
        strictly_between(top(cx, "row-2"), 72.0, 144.0),
        "following the slot"
    );
    // The last 60 Hz frame of the 150 ms close.
    advance(cx, ms(133.333 - 75.0));
    assert!(list.read_with(cx, |l, _| l.exit.is_some()), "still closing");
    let last_motion_frame = list_height(cx);
    advance(cx, tokens::FIRST_STEP);
    assert!(
        list.read_with(cx, |l, _| l.exit.is_none()),
        "done after 150 ms"
    );
    assert_eq!(top(cx, "row-2"), 72.0, "at the removed row's place");
    assert_eq!(list_height(cx), 132.0);
    assert_eq!(last_motion_frame, list_height(cx), "no jump at the end");
}

#[gpui_kit::test]
fn reduced_in_flow_exit_fades_then_closes(cx: &mut TestAppContext) {
    cx.update(|cx| motion::set_override(Some(MotionPolicy::Reduced), cx));
    let (list, cx) = open_list(cx);
    assert_eq!(top(cx, "copy-1"), 72.0);
    step_to(cx, ms(50.0));
    let at = list.read_with(cx, |l, _| l.last).expect("fading");
    assert!(strictly_between(at.opacity, 0.0, 1.0), "{at:?}");
    assert_eq!(at.offset, point(px(0.), px(0.)));
    assert_eq!(top(cx, "row-2"), 144.0, "the slot holds while it fades");
    advance(cx, ms(66.667));
    assert!(list.read_with(cx, |l, _| l.exit.is_none()));
    assert_eq!(top(cx, "row-2"), 72.0, "then closes at once");
}

// ---------------------------------------------------------------------------
// Policy switches mid-motion (ADR-0030, Motion policy).

#[gpui_kit::test]
fn switch_to_reduced_mid_motion_zeroes_travel_and_lets_opacity_finish(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    // An open.
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    assert!(button(cx).origin.x < px(300.));
    cx.update(|_, cx| cx.set_reduce_motion(true));
    frame(cx);
    let switched = drawn(cx, &probe).0;
    assert_eq!(button(cx).origin.x, px(300.), "no more travel: at its end");
    assert!(strictly_between(switched.opacity, 0.0, 1.0), "{switched:?}");
    advance(cx, tokens::QUICK);
    let end = drawn(cx, &probe).0;
    assert_eq!((end.value, end.opacity, end.settled), (1.0, 1.0, true));

    // A close.
    cx.update(|_, cx| cx.set_reduce_motion(false));
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(60.0));
    let reached = button(cx).origin.x;
    assert!(px(100.) < reached && reached < px(300.));
    cx.update(|_, cx| cx.set_reduce_motion(true));
    frame(cx);
    advance(cx, tokens::FIRST_STEP);
    assert_eq!(button(cx).origin.x, reached, "it holds where it was");
    let fading = drawn(cx, &probe).0;
    assert!(strictly_between(fading.opacity, 0.0, 1.0), "{fading:?}");
    advance(cx, tokens::MICRO);
    assert_eq!(button(cx).origin.x, px(100.), "then snaps");
}

#[gpui_kit::test]
fn switch_to_full_never_restarts_a_settled_motion(cx: &mut TestAppContext) {
    let (_host, probe, cx) = open_probe(cx, false);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    toggle(cx, &probe, Initiator::Pointer);
    step_to(cx, ms(75.0));
    assert_eq!(button(cx).origin.x, px(300.), "a Reduced fade");
    // Mid-fade, then settled: the switch moves nothing.
    cx.update(|_, cx| cx.set_reduce_motion(false));
    frame(cx);
    assert_eq!(
        button(cx).origin.x,
        px(300.),
        "the fade is not restarted as a slide"
    );
    advance(cx, tokens::QUICK);
    assert!(drawn(cx, &probe).0.settled);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    frame(cx);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    frame(cx);
    assert!(drawn(cx, &probe).0.settled);
    assert_eq!(button(cx).origin.x, px(300.));
    assert_eq!(requested_frames(cx), 0, "nothing runs");
}

// ---------------------------------------------------------------------------
// gpui-kit's tokens.

#[gpui_kit::test]
fn kit_tokens_have_no_ease_in_and_no_overshoot(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let tokens = motion::kit_tokens();
    for (name, easing) in [
        ("enter", &tokens.easing_enter),
        ("exit", &tokens.easing_exit),
    ] {
        assert!(
            easing.sample(0.1) >= 0.3,
            "{name} eases out: {}",
            easing.sample(0.1)
        );
    }
    for spring in [tokens.spring_control, tokens.spring_move] {
        assert!(format!("{spring:?}").contains("damping: 1.0"), "{spring:?}");
    }
    assert_eq!(
        (
            tokens.duration_fast,
            tokens.duration_normal,
            tokens.duration_slow
        ),
        (ms(150.0), ms(200.0), ms(240.0)),
        "QUICK, BASE, PANEL"
    );

    // The app writes them into the kit's theme, and keeps them across a
    // theme change.
    let shell = start(cx);
    for theme in ["Pierre Dark", "Polygloss Light"] {
        assert!(
            shell
                .cx
                .update(|_, cx| polygloss_app::theme::apply_theme(theme, cx))
        );
        let exit_at = shell.cx.update(|_, cx| {
            gpui_kit::component::theme::Theme::global(cx)
                .motion
                .easing_exit
                .sample(0.1)
        });
        assert!(exit_at >= 0.3, "{theme}: {exit_at}");
        let switch = shell.cx.update(|_, cx| {
            gpui_kit::component::theme::Theme::global(cx)
                .motion
                .spring_move
        });
        assert!(
            format!("{switch:?}").contains("damping: 1.0"),
            "{theme}: {switch:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The M6 entrances (`enter_from`), on `Track` until T7.9 and T7.10 move
// their callers.

/// A window drawing one 40 × 20 pt box that enters from 12 pt right of its
/// place, as the threads panel's content does.
struct Entering {
    epoch: u64,
}

impl Render for Entering {
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

/// What the motions sampled, in order.
type Drawn = Rc<RefCell<Vec<Recorded>>>;

fn record(cx: &mut TestAppContext) -> Drawn {
    let drawn: Drawn = Rc::default();
    cx.update(|cx| motion::record(drawn.clone(), cx));
    drawn
}

/// The last opacity recorded for `id` since `drawn` was cleared.
fn opacity_of(drawn: &Drawn, id: &ElementId) -> Option<f32> {
    drawn
        .borrow()
        .iter()
        .rev()
        .find(|r| &r.id == id)
        .map(|r| r.opacity)
}

fn open_entering(cx: &mut TestAppContext) -> (Entity<Entering>, &mut VisualTestContext) {
    cx.add_window_view(|_, _| Entering { epoch: 1 })
}

/// Draws one frame at the clock's current time.
fn entering_frame(cx: &mut VisualTestContext, drawn: &Drawn) {
    drawn.borrow_mut().clear();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// From the commit frame, the frame `since_commit` later by the stepping
/// protocol (a first step of 16.667 ms, then the rest).
fn entering_step(cx: &mut VisualTestContext, drawn: &Drawn, since_commit: Duration) {
    cx.executor().advance_clock(tokens::FIRST_STEP);
    entering_frame(cx, drawn);
    cx.executor()
        .advance_clock(since_commit - tokens::FIRST_STEP);
    entering_frame(cx, drawn);
}

/// The probe's offset from its place and its opacity, as last drawn.
fn entering_sample(cx: &mut VisualTestContext, drawn: &Drawn) -> (Pixels, f32) {
    let bounds = cx.debug_bounds("probe").expect("the probe is painted");
    assert_eq!(bounds.origin.y, px(0.), "the probe moves sideways only");
    let opacity = opacity_of(drawn, &ElementId::from("probe")).expect("the probe entered");
    (bounds.origin.x, opacity)
}

#[gpui_kit::test]
fn enter_from_starts_at_full_travel_and_settles_on_the_test_clock(cx: &mut TestAppContext) {
    let drawn = record(cx);
    let (_probe, cx) = open_entering(cx);
    entering_frame(cx, &drawn);
    assert_eq!(
        entering_sample(cx, &drawn),
        (px(12.), 0.0),
        "the first frame"
    );

    cx.executor().advance_clock(Duration::from_millis(90));
    entering_frame(cx, &drawn);
    let (x, opacity) = entering_sample(cx, &drawn);
    assert!(strictly_between(x.as_f32(), 0., 12.), "half way: x {x:?}");
    assert!(strictly_between(opacity, 0., 1.), "half way: {opacity}");

    cx.executor().advance_clock(Duration::from_millis(180));
    entering_frame(cx, &drawn);
    assert_eq!(entering_sample(cx, &drawn), (px(0.), 1.0), "settled");
}

#[gpui_kit::test]
fn enter_from_replays_only_for_a_new_epoch(cx: &mut TestAppContext) {
    let drawn = record(cx);
    let (probe, cx) = open_entering(cx);
    entering_frame(cx, &drawn);
    entering_step(cx, &drawn, Duration::from_millis(200));
    assert_eq!(entering_sample(cx, &drawn), (px(0.), 1.0));

    // Drawn again with the same epoch: it stays where it is.
    entering_frame(cx, &drawn);
    entering_frame(cx, &drawn);
    assert_eq!(entering_sample(cx, &drawn), (px(0.), 1.0), "no replay");

    // A new epoch enters again from the start.
    probe.update(cx, |p, cx| {
        p.epoch = 2;
        cx.notify();
    });
    entering_frame(cx, &drawn);
    assert_eq!(entering_sample(cx, &drawn), (px(12.), 0.0), "replayed");
    entering_step(cx, &drawn, Duration::from_millis(180));
    assert_eq!(entering_sample(cx, &drawn), (px(0.), 1.0));
}

#[gpui_kit::test]
fn enter_from_snaps_under_reduced(cx: &mut TestAppContext) {
    for policy in [Some(MotionPolicy::Reduced), Some(MotionPolicy::Off), None] {
        let drawn = record(cx);
        cx.update(|cx| match policy {
            Some(policy) => motion::set_override(Some(policy), cx),
            // Reduce Motion as the system reports it.
            None => cx.set_reduce_motion(true),
        });
        let (_probe, vcx) = open_entering(cx);
        entering_frame(vcx, &drawn);
        assert_eq!(
            entering_sample(vcx, &drawn),
            (px(0.), 1.0),
            "{policy:?}: its first frame"
        );
        cx.update(|cx| {
            motion::set_override(None, cx);
            cx.set_reduce_motion(false);
        });
    }
}

#[gpui_kit::test]
fn initiator_follows_the_last_input(cx: &mut TestAppContext) {
    let (_probe, cx) = open_entering(cx);
    let initiator = |cx: &mut VisualTestContext| cx.update(|window, _| motion::initiator(window));
    let pointer =
        |cx: &mut VisualTestContext| cx.update(|window, _| motion::pointer_initiated(window));
    cx.simulate_keystrokes("a");
    assert_eq!(initiator(cx), Initiator::Keyboard, "a key was last");
    assert!(!pointer(cx));
    cx.simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::none());
    assert_eq!(initiator(cx), Initiator::Pointer, "the pointer was last");
    assert!(pointer(cx));
    cx.simulate_keystrokes("b");
    assert_eq!(initiator(cx), Initiator::Keyboard);
    cx.simulate_click(point(px(6.), px(6.)), Modifiers::none());
    assert_eq!(initiator(cx), Initiator::Pointer);
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
    assert_eq!(
        shell.cx.update(|_, cx| motion::policy(cx)),
        MotionPolicy::Reduced
    );
    let (probe, probe_window) = {
        let (probe, pcx) = shell.cx.add_window_view(|_, _| Entering { epoch: 1 });
        entering_frame(pcx, &drawn);
        assert_eq!(
            entering_sample(pcx, &drawn),
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
    entering_frame(pcx, &drawn);
    assert_eq!(
        entering_sample(pcx, &drawn),
        (px(12.), 0.0),
        "it moves again"
    );
}

/// The threads panel's content: its left edge's offset from the pane's, and
/// the opacity its entrance drew on the last frame (`None`: no entrance was
/// drawn since `drawn` was last cleared).
fn panel_sample(cx: &mut VisualTestContext, drawn: &Drawn) -> (f32, Option<f32>) {
    let pane = crate::shell::bounds(cx, "threads-pane");
    let content = crate::shell::bounds(cx, "threads-panel");
    let opacity = opacity_of(drawn, &ElementId::from("threads-panel"));
    ((content.left() - pane.left()).as_f32(), opacity)
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
    entering_frame(shell.cx, &drawn);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let (start_x, start_opacity) = panel_sample(shell.cx, &drawn);
    shell.cx.executor().advance_clock(Duration::from_millis(90));
    entering_frame(shell.cx, &drawn);
    let (mid_x, mid_opacity) = panel_sample(shell.cx, &drawn);
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(180));
    entering_frame(shell.cx, &drawn);
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
    entering_frame(shell.cx, &drawn);
    assert_eq!(panel_sample(shell.cx, &drawn), (end_x, None), "no replay");

    // Closing is instant; opening again enters again.
    click_threads_button(shell.cx);
    entering_frame(shell.cx, &drawn);
    assert!(crate::shell::painted(shell.cx, "threads-pane").is_none());
    click_threads_button(shell.cx);
    entering_frame(shell.cx, &drawn);
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
    entering_frame(shell.cx, &drawn);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let (at_once, entrance) = panel_sample(shell.cx, &drawn);
    assert_eq!(entrance, None, "no entrance");
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(500));
    entering_frame(shell.cx, &drawn);
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
    entering_frame(shell.cx, &drawn);
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
    entering_frame(shell.cx, &drawn);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let (first, opacity) = panel_sample(shell.cx, &drawn);
    assert_eq!(opacity.unwrap_or(1.0), 1.0, "opaque on its first frame");
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(500));
    entering_frame(shell.cx, &drawn);
    assert_eq!(panel_sample(shell.cx, &drawn).0, first, "never moved");
}
