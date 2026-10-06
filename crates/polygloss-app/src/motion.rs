//! Motion in the app (design §11.16, ADR-0030), on the viewport's core
//! (`polygloss_viewport::motion`, re-exported here): the policy override
//! for harnesses, [`sample`] on the executor clock, [`initiator`],
//! [`settle`] (what settles or freezes running motion), the [`wrap`]
//! elements, [`exit`] (a surface leaving after its model), press [`ink`]
//! and gpui-kit's motion tokens ([`kit_tokens`]).
//!
//! Every motion is a [`Track`] field of the entity that owns its surface,
//! sampled in that entity's render (rule 13). [`enter_from`] carries the two
//! M6 entrances until T7.9 and T7.10 move their callers onto tracks of their
//! own; it maps Reduced to a snap, as in M6. macOS Reduce Motion is read
//! again on every activation of the main window
//! ([`follow_system_reduce_motion`]).

pub mod exit;
pub mod settle;
pub mod wrap;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{Easing, Spring};
use gpui_kit::component::MotionTokens;
use gpui_kit::component::theme::Theme;
use gpui_kit::{
    AnyElement, App, Context, ElementId, EntityId, Global, IntoElement, Pixels, Point, Styled,
    Window, point,
};
pub use polygloss_viewport::motion::*;

use crate::window::MainWindow;

/// The threads panel's content entering (M6; retired by T7.10).
pub const ENTER_PANEL: Duration = Duration::from_millis(180);
/// A banner notice entering (M6; retired by T7.9).
pub const ENTER_NOTICE: Duration = Duration::from_millis(160);

/// Ease-out quint: fast, then a long settle (the threads panel, M6).
pub fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(5)
}

/// Ease-out cubic (banner notices, M6).
pub fn ease_out_cubic(t: f32) -> f32 {
    gpui_kit::base::animation::ease_out_cubic(t)
}

/// Whether what the user did last was with the pointer.
pub fn pointer_initiated(window: &Window) -> bool {
    initiator(window) == Initiator::Pointer
}

/// Who caused the change being handled: the keyboard or the pointer,
/// whichever the user used last. Call it inside a user-input handler only;
/// restores, jumps and the rest pass [`Initiator::Programmatic`].
pub fn initiator(window: &Window) -> Initiator {
    if window.last_input_was_keyboard() {
        Initiator::Keyboard
    } else {
        Initiator::Pointer
    }
}

/// Samples `track` at the executor clock's now under the current policy
/// (a switch mid-motion: [`Track::follow_policy`]), requesting the next
/// frame while it runs, never while it is frozen or settled. Call it from
/// the owner's render.
pub fn sample(track: &mut Track, window: &mut Window, cx: &App) -> Sample {
    let now = cx.background_executor().now();
    track.follow_policy(policy(cx), now);
    let sample = track.sample(now);
    if !sample.settled && !track.is_frozen() {
        window.request_animation_frame();
    }
    sample
}

/// `App::reduce_motion` as it was before [`set_override`] set it.
struct SavedReduceMotion(bool);

impl Global for SavedReduceMotion {}

/// Harnesses only: makes `policy` the app's motion policy (`None`: follow
/// the system again). Any override also sets `App::reduce_motion`, so
/// gpui-kit's motion settles whatever policy a test samples; clearing it
/// restores the flag it found.
pub fn set_override(policy: Option<MotionPolicy>, cx: &mut App) {
    match policy {
        Some(_) => {
            if cx.try_global::<SavedReduceMotion>().is_none() {
                let saved = cx.reduce_motion();
                cx.set_global(SavedReduceMotion(saved));
            }
            cx.set_reduce_motion(true);
        }
        None => {
            if cx.has_global::<SavedReduceMotion>() {
                let SavedReduceMotion(saved) = cx.remove_global::<SavedReduceMotion>();
                cx.set_reduce_motion(saved);
            }
        }
    }
    cx.set_global(MotionPolicyOverride(policy));
}

/// What a motion painted on one frame (tests: opacity and rotation are
/// not observable otherwise).
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub id: ElementId,
    pub offset: Point<Pixels>,
    pub opacity: f32,
    /// Degrees.
    pub rotation: f32,
}

/// Where [`record`] collects.
struct RecordSink(Rc<RefCell<Vec<Recorded>>>);

impl Global for RecordSink {}

/// Test seam: every motion sample from now on is appended to `sink`.
pub fn record(sink: Rc<RefCell<Vec<Recorded>>>, cx: &mut App) {
    cx.set_global(RecordSink(sink));
}

/// Appends `recorded` to the [`record`] sink, if a test installed one.
pub(crate) fn report(recorded: impl FnOnce() -> Recorded, cx: &App) {
    if let Some(RecordSink(sink)) = cx.try_global::<RecordSink>() {
        sink.borrow_mut().push(recorded());
    }
}

/// The M6 entrances [`enter_from`] plays: by the rendering view and the
/// caller's id, the epoch it played and its track (retired with
/// `enter_from`).
#[derive(Default)]
struct Entrances(HashMap<(EntityId, ElementId), (u64, Track)>);

impl Global for Entrances {}

/// `element` entering (M6): drawn `from` its place at opacity 0, then easing
/// to its place at opacity 1 over `duration`, on a [`Track`]. It plays once
/// per `(view, id, epoch)`: drawn again with the same epoch it stays put; a
/// new epoch plays again. Pointer only, and Reduced snaps, as in M6.
#[allow(clippy::too_many_arguments)]
pub fn enter_from(
    id: impl Into<ElementId>,
    epoch: u64,
    from: Point<Pixels>,
    duration: Duration,
    easing: fn(f32) -> f32,
    element: impl IntoElement + Styled,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let key = (window.current_view(), id.clone());
    let now = cx.background_executor().now();
    let policy = policy(cx);
    let entrances = &mut cx.default_global::<Entrances>().0;
    let (played, track) = entrances
        .entry(key.clone())
        .or_insert_with(|| (epoch.wrapping_add(1), Track::new(1.0, 1.0)));
    if *played != epoch {
        let motion = Motion {
            enter: duration,
            exit: duration,
            easing,
            animates: &[Initiator::Pointer],
            reduced: Reduced {
                enter: ReducedPlay::Snap,
                exit: ReducedPlay::Snap,
            },
        };
        *played = epoch;
        *track = Track::new(0.0, 1.0);
        track.retarget(1.0, &motion, Initiator::Pointer, policy, now);
    }
    let mut track = track.clone();
    let progress = sample(&mut track, window, cx);
    if let Some((_, kept)) = cx.default_global::<Entrances>().0.get_mut(&key) {
        *kept = track;
    }
    let progress = progress.value;
    let offset = point(from.x * (1.0 - progress), from.y * (1.0 - progress));
    report(
        || Recorded {
            id,
            offset,
            opacity: progress,
            rotation: 0.0,
        },
        cx,
    );
    element
        .relative()
        .left(offset.x)
        .top(offset.y)
        .opacity(progress)
        .into_any_element()
}

/// Press and hover feedback (ADR-0030, Feedback without motion): instant,
/// color only.
pub mod ink {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{App, StatefulInteractiveElement, Styled};

    /// Hover: the foreground at this alpha over the control.
    pub const HOVER: f32 = 0.06;
    /// Pressed: the foreground at this alpha.
    pub const PRESSED: f32 = 0.12;

    /// Hover and pressed ink on a custom control (one with an element id:
    /// gpui keeps the pressed state in element state), from the mouse down
    /// to the mouse up. Set no other hover style on it.
    pub trait PressInk: Styled + StatefulInteractiveElement {
        fn press_ink(self, cx: &App) -> Self {
            let foreground = cx.theme().foreground;
            self.hover(|style| style.bg(foreground.opacity(HOVER)))
                .active(|style| style.bg(foreground.opacity(PRESSED)))
        }
    }

    impl<E: Styled + StatefulInteractiveElement> PressInk for E {}
}

/// gpui-kit's motion tokens from ADR-0030's: durations QUICK, BASE and
/// PANEL, OUT to enter and exit (no ease-in), MOVE to move, and springs that
/// never overshoot (damping 1.0; the Switch's thumb).
pub fn kit_tokens() -> MotionTokens {
    let out = Easing::Custom(Rc::new(tokens::out));
    MotionTokens {
        duration_instant: Duration::ZERO,
        duration_fast: tokens::QUICK,
        duration_normal: tokens::BASE,
        duration_slow: tokens::PANEL,
        easing_enter: out.clone(),
        easing_exit: out,
        easing_move: Easing::Custom(Rc::new(tokens::in_out)),
        spring_control: Spring::new(Duration::from_millis(180)).with_damping(1.0),
        spring_move: Spring::new(tokens::PANEL)
            .with_damping(1.0)
            .with_epsilon(0.1),
        ..MotionTokens::default()
    }
}

/// Writes [`kit_tokens`] into gpui-kit's theme (after each theme applies).
pub fn apply_kit_tokens(cx: &mut App) {
    Theme::update(cx, |kit| kit.motion = kit_tokens());
}

/// Reads macOS Reduce Motion into `App::reduce_motion` (a GPUI global; by
/// default gpui-kit's `apply_system_reduce_motion`, which does nothing
/// under the test scheduler, so tests install their own).
#[derive(Clone, Copy)]
pub struct ReduceMotionSource(pub fn(&mut App));

impl Default for ReduceMotionSource {
    fn default() -> Self {
        ReduceMotionSource(gpui_kit::base::apply_system_reduce_motion)
    }
}

impl Global for ReduceMotionSource {}

/// Reads Reduce Motion again (the main window was activated: the setting may
/// have changed while the app was in the background) and redraws `window`
/// when it changed.
pub fn follow_system_reduce_motion(window: &mut Window, cx: &mut App) {
    let ReduceMotionSource(read) = cx
        .try_global::<ReduceMotionSource>()
        .copied()
        .unwrap_or_default();
    let before = cx.reduce_motion();
    read(cx);
    if cx.reduce_motion() != before {
        window.refresh();
    }
}

/// Settling (the keystroke interceptor) and Reduce Motion followed on every
/// activation of each main window.
pub fn init(cx: &mut App) {
    settle::init(cx);
    cx.observe_new(
        |_: &mut MainWindow, window: Option<&mut Window>, cx: &mut Context<MainWindow>| {
            let Some(window) = window else {
                return;
            };
            cx.observe_window_activation(window, |_, window, cx| {
                if window.is_window_active() {
                    follow_system_reduce_motion(window, cx);
                }
            })
            .detach();
        },
    )
    .detach();
}
