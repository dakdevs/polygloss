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
//!
//! The root also counts the frames its window paints ([`painted_frames`]):
//! a surface that was not drawn on the last of them is off screen (a hidden
//! tab), so it starts no exit and drops one under way (rule 9).

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
    /// Frames painted so far.
    painted: u64,
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

/// How many frames `window` has painted (0 in a window without a
/// [`root`]). A render reads the frames before its own; a surface whose last
/// render read `n` was drawn on the window's latest frame while this reads
/// `n + 1`, in a handler or in its next render.
pub fn painted_frames(window: &Window, cx: &App) -> u64 {
    cx.try_global::<Registries>()
        .and_then(|r| r.0.get(&window.window_handle().window_id()))
        .map_or(0, |r| r.painted)
}

/// Whether a surface is on screen, for one that starts or keeps motion
/// only while it is (rule 9: an exit never plays because a tab switch hid
/// its surface): its owner calls [`OnScreen::mark`] from every render.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnScreen {
    /// [`painted_frames`] as the surface's last render read it.
    rendered: Option<u64>,
}

impl OnScreen {
    /// Whether the surface was drawn on its window's latest painted frame:
    /// in a handler, whether it is on screen; in its render (before
    /// [`Self::mark`]), whether it was on the frame before this one.
    pub fn was_drawn(&self, window: &Window, cx: &App) -> bool {
        self.rendered
            .is_some_and(|n| painted_frames(window, cx) == n + 1)
    }

    /// The surface renders on this frame.
    pub fn mark(&mut self, window: &Window, cx: &App) {
        self.rendered = Some(painted_frames(window, cx));
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
        current.painted += 1;
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
        // What the keystroke runs where the keyboard is: the binding the
        // focused element's context path resolves it to (the rendered
        // frame's own context stack is wherever its last element left it,
        // so `Window::bindings_for_action` cannot answer this).
        let bound = cx
            .key_bindings()
            .borrow()
            .bindings_for_input(
                std::slice::from_ref(&event.keystroke),
                &window.context_stack(),
            )
            .0
            .into_iter()
            .next();
        let registry = registry(window, cx);
        let all: Vec<_> = registry
            .live
            .iter()
            .chain(&registry.frozen)
            .cloned()
            .collect();
        for registration in all {
            let own_shortcut = registration
                .action
                .as_ref()
                .zip(bound.as_ref())
                .is_some_and(|(action, binding)| binding.action().partial_eq(action.as_ref()));
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
