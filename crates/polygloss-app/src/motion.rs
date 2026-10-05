//! Product motion (design §11.16, ADR-0029): the durations and easings of
//! the app's two entrances, [`enter_from`] to draw one, [`pointer_initiated`]
//! to tell a pointer open from a keyboard one, and macOS Reduce Motion read
//! again on every activation of the main window
//! ([`follow_system_reduce_motion`]).
//!
//! Motion runs on gpui-base's executor clock (`animate_keyframes`), never on
//! gpui's wall-clock `with_animation`, so tests step it with
//! `advance_clock`. With Reduce Motion on, every entrance shows its end on
//! its first frame. This module moves nothing itself: the threads panel
//! (T6.12) and banner notices (T6.13) call [`enter_from`].

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::motion::{Easing, Keyframe, Keyframes, Timing, animate_keyframes};
use gpui_kit::{
    AnyElement, App, Context, ElementId, Global, IntoElement, Pixels, Point, SharedString, Styled,
    Window, point,
};

use crate::window::MainWindow;

/// The threads panel's content entering (design §11.16).
pub const ENTER_PANEL: Duration = Duration::from_millis(180);
/// A banner notice entering (design §11.16).
pub const ENTER_NOTICE: Duration = Duration::from_millis(160);

/// Ease-out quint: fast, then a long settle (the threads panel).
pub fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(5)
}

/// Ease-out cubic (banner notices).
pub fn ease_out_cubic(t: f32) -> f32 {
    gpui_kit::base::animation::ease_out_cubic(t)
}

/// Whether what the user did last was with the pointer: an entrance opened
/// from the keyboard shows its end at once (ADR-0029).
pub fn pointer_initiated(window: &Window) -> bool {
    !window.last_input_was_keyboard()
}

/// What an entrance drew on a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entrance {
    /// Where the element was drawn from its place.
    pub offset: Point<Pixels>,
    pub opacity: f32,
}

/// `element` entering: drawn `from` its place (relative to it) at opacity
/// 0, then easing to its place at opacity 1 over `duration`, on the
/// executor clock. The playback is keyed by `(id, epoch)` among the
/// elements around it, so it plays once per epoch while the element is
/// drawn on every frame; a new epoch plays it again. Under Reduce Motion
/// the first frame is the end.
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
    let key = ElementId::NamedChild(Arc::new(id.clone()), SharedString::from(epoch.to_string()));
    let track = Keyframes::try_new([Keyframe::new(0.0, 0.0_f32), Keyframe::new(1.0, 1.0_f32)])
        .expect("two keyframes from 0 to 1");
    let timing = Timing::new(duration).ease(Easing::Custom(Rc::new(easing)));
    let progress = animate_keyframes(key, &track, timing, window, cx).value;
    let entrance = Entrance {
        offset: point(from.x * (1.0 - progress), from.y * (1.0 - progress)),
        opacity: progress,
    };
    if let Some(EntranceSink(sink)) = cx.try_global::<EntranceSink>() {
        sink.clone()(&id, entrance);
    }
    element
        .relative()
        .left(entrance.offset.x)
        .top(entrance.offset.y)
        .opacity(entrance.opacity)
        .into_any_element()
}

/// What [`record_entrances`] calls.
type Sink = Rc<dyn Fn(&ElementId, Entrance)>;

/// Where [`enter_from`] reports what it drew (a GPUI global).
struct EntranceSink(Sink);

impl Global for EntranceSink {}

/// Reports every entrance [`enter_from`] draws, by its id, to `sink`
/// (tests: an element's opacity is not observable from outside).
pub fn record_entrances(sink: impl Fn(&ElementId, Entrance) + 'static, cx: &mut App) {
    cx.set_global(EntranceSink(Rc::new(sink)));
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

/// Follows Reduce Motion on every activation of each main window.
pub fn init(cx: &mut App) {
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
