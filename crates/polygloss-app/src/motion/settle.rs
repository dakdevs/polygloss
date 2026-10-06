//! What settles or freezes running motion (ADR-0030 rule 4), per main
//! window. Owners [`register`] from render while their motion runs; the
//! registry is rebuilt every frame.
//!
//! - A key down settles every registration before the key is handled: a
//!   keystroke interceptor ([`init`]) runs before action bindings, which
//!   key listeners never do. A lone modifier and a keystroke bound to a
//!   registration's own action (whose handler retargets it) settle nothing
//!   of that registration.
//! - A mouse down freezes every registration, so the click lands on what
//!   was painted where it was pressed; after its mouse up, and after the
//!   effects the click queued, each registration it froze settles
//!   ([`Settle::Frozen`]: only a track still frozen). [`root`] listens for
//!   both, and for the scroll wheel ([`Settle::All`]).
//! - A resize, fullscreen or a scale change settles everything
//!   ([`bounds_observer`]); a window move does not.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{
    Action, AnyElement, App, Context, DispatchPhase, Element, ElementId, Global, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, MouseDownEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, Style, Window, WindowId,
};

use super::Settle;

type Freeze = Box<dyn Fn(Instant, &mut App)>;
type SettleFn = Box<dyn Fn(Settle, &mut App)>;

/// One owner's running motion.
struct Registration {
    /// The motion's toggle: its own shortcut retargets instead of settling.
    action: Option<Box<dyn Action>>,
    freeze: Freeze,
    settle: SettleFn,
}

/// A window's registrations.
#[derive(Default)]
struct Registry {
    /// Whether the window paints a [`root`]: only then is `next` consumed.
    rooted: bool,
    /// Registered since the last frame was painted.
    next: Vec<Rc<Registration>>,
    /// The last painted frame's.
    live: Vec<Rc<Registration>>,
    /// What the pressed mouse froze: kept past the per-frame rebuild, so an
    /// owner that does not render again (a cached view) still settles.
    frozen: Vec<Rc<Registration>>,
}

#[derive(Default)]
struct Registries(HashMap<WindowId, Registry>);

impl Global for Registries {}

fn registry<'a>(window: &Window, cx: &'a mut App) -> &'a mut Registry {
    cx.default_global::<Registries>()
        .0
        .entry(window.window_handle().window_id())
        .or_default()
}

/// Registers running motion for this frame: call from the owner's render
/// while its motion is unsettled, in a window that paints a [`root`]
/// (elsewhere it registers nothing). `action` is the motion's toggle (its own
/// shortcut retargets); `freeze` and `settle` act on the owner's tracks and
/// notify it.
pub fn register(
    action: Option<Box<dyn Action>>,
    freeze: impl Fn(Instant, &mut App) + 'static,
    settle: impl Fn(Settle, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let registry = registry(window, cx);
    if registry.rooted {
        registry.next.push(Rc::new(Registration {
            action,
            freeze: Box::new(freeze),
            settle: Box::new(settle),
        }));
    }
}

/// Settles every registration of `window` (the last frame's and those a
/// mouse down froze).
fn settle_all(window: &Window, cx: &mut App) {
    let registry = registry(window, cx);
    let all: Vec<_> = registry
        .live
        .iter()
        .chain(&registry.frozen)
        .cloned()
        .collect();
    for registration in all {
        (registration.settle)(Settle::All, cx);
    }
}

/// The settle root: `MainWindow`'s first child (and any window's that runs
/// motion), painted first every frame. It takes no space.
pub fn root() -> AnyElement {
    SettleRoot.into_any_element()
}

struct SettleRoot;

impl IntoElement for SettleRoot {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for SettleRoot {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: gpui_kit::Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui_kit::Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui_kit::Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // This frame's registrations are complete: every owner rendered
        // before paint.
        let current = registry(window, cx);
        current.rooted = true;
        current.live = std::mem::take(&mut current.next);
        // Mouse listeners last one frame: register them every frame.
        window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let now = cx.background_executor().now();
            let registry = registry(window, cx);
            let live = registry.live.clone();
            for registration in &live {
                if !registry
                    .frozen
                    .iter()
                    .any(|frozen| Rc::ptr_eq(frozen, registration))
                {
                    registry.frozen.push(registration.clone());
                }
            }
            for registration in live {
                (registration.freeze)(now, cx);
            }
        });
        window.on_mouse_event(|_: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let frozen = std::mem::take(&mut registry(window, cx).frozen);
            if frozen.is_empty() {
                return;
            }
            // Behind the click's own effects: `Window::dispatch_action` is
            // itself deferred, so defer twice.
            cx.defer(move |cx| {
                cx.defer(move |cx| {
                    for registration in frozen {
                        (registration.settle)(Settle::Frozen, cx);
                    }
                })
            });
        });
        window.on_mouse_event(|_: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                settle_all(window, cx);
            }
        });
    }
}

/// Whether `keystroke` is a lone modifier's release, which gpui reports as
/// a keystroke named after the modifier.
fn lone_modifier(keystroke: &gpui_kit::Keystroke) -> bool {
    keystroke.key_char.is_none()
        && matches!(
            keystroke.key.as_str(),
            "shift" | "control" | "alt" | "platform" | "function"
        )
}

/// A key down settles every registration of its window before bindings
/// run, except a lone modifier and a registration whose own action the
/// keystroke is bound to.
pub fn init(cx: &mut App) {
    cx.intercept_keystrokes(|event, window, cx| {
        if lone_modifier(&event.keystroke) {
            return;
        }
        let registry = registry(window, cx);
        let all: Vec<_> = registry
            .live
            .iter()
            .chain(&registry.frozen)
            .cloned()
            .collect();
        for registration in all {
            let own_shortcut = registration.action.as_ref().is_some_and(|action| {
                window
                    .bindings_for_action(action.as_ref())
                    .iter()
                    .any(|binding| {
                        binding.match_keystrokes(std::slice::from_ref(&event.keystroke))
                            == Some(false)
                    })
            });
            if !own_shortcut {
                (registration.settle)(Settle::All, cx);
            }
        }
    })
    .detach();
}

/// The callback for `cx.observe_window_bounds` in a window's root view
/// (`MainWindow::new`): settles everything when the content size or the
/// scale factor changed (a resize, fullscreen, another display). gpui also
/// reports a window move, which settles nothing.
pub fn bounds_observer<T: 'static>(
    window: &Window,
) -> impl FnMut(&mut T, &mut Window, &mut Context<T>) + 'static {
    let mut last = (window.viewport_size(), window.scale_factor());
    move |_, window, cx| {
        let now = (window.viewport_size(), window.scale_factor());
        if now != last {
            last = now;
            settle_all(window, cx);
        }
    }
}
