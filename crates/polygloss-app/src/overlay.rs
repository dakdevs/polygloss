//! App-owned overlays (ADR-0030 "Inherited gpui-kit motion", OQ-49): the
//! command palette (⌘K), the file finder (⌘P) and the cheat sheet (`?`)
//! open and close on one frame for every initiator, where gpui-kit's
//! dialogs slide and fade in over 250 ms.
//!
//! One overlay is open at a time, drawn by [`crate::window::MainWindow`]
//! above its page (Home or a review) and under gpui-kit's root layers (its
//! dialogs and toasts): a backdrop over the whole window and a frame in
//! gpui-kit's dialog look (its background, border and shadow at rest,
//! radius `LG`), centred, at its [`Frame`]'s offset from the window's top.
//! Like a kit dialog, the frame keeps `CARD_X` of the window clear and moves
//! up rather than run off the bottom. Focus moves into the overlay when it
//! opens and is trapped there (⇥ cycles inside it); Esc (gpui-base's
//! `Dialog` bindings, as in the kit's dialogs) and a click outside the frame
//! close it, and the keyboard goes back where it was. Nothing beneath takes
//! a click or a key while it is open.

use std::rc::Rc;

use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::base::actions::{Cancel, Confirm};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, FocusHandle, Global, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Role, StatefulInteractiveElement as _, Styled as _,
    WeakFocusHandle, Window, div, px,
};

use crate::space::{edge, layout, radius};

/// Where an overlay's frame sits: its width and its offset from the
/// window's top (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub width: f32,
    pub top: f32,
}

impl Frame {
    /// The palette's and the finder's (ADR-0031: the pickers' frame).
    pub const PICKER: Frame = Frame {
        width: layout::PICKER_W,
        top: layout::PICKER_TOP,
    };
}

/// An overlay [`open_overlay`] opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayHandle(u64);

impl OverlayHandle {
    /// Whether this overlay is still open (closed, or replaced by another).
    pub fn is_open(self, cx: &App) -> bool {
        open(cx).is_some_and(|open| open.handle == self)
    }
}

/// What draws an overlay's content, each frame.
type Render = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The open overlay.
struct Open {
    handle: OverlayHandle,
    window: AnyWindowHandle,
    frame: Frame,
    /// The overlay's own focus: the trap holds it, Esc reaches it.
    focus: FocusHandle,
    /// Where the keyboard goes back to when it closes.
    previous: Option<WeakFocusHandle>,
    render: Render,
}

#[derive(Default)]
struct Overlays {
    open: Option<Open>,
    opened: u64,
}

impl Global for Overlays {}

/// The open overlay, unless its window has closed.
fn open(cx: &App) -> Option<&Open> {
    let open = cx.try_global::<Overlays>()?.open.as_ref()?;
    cx.windows().contains(&open.window).then_some(open)
}

/// Whether an overlay is open.
pub fn is_open(cx: &App) -> bool {
    open(cx).is_some()
}

/// Opens `render`'s content over `window` in the pickers' frame
/// ([`Frame::PICKER`]), focused.
pub fn open_overlay(
    window: &mut Window,
    cx: &mut App,
    render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> OverlayHandle {
    open_overlay_in(Frame::PICKER, window, cx, render)
}

/// Opens `render`'s content over `window` in `frame`, on the next frame,
/// and moves the keyboard into it (callers then focus their search field).
/// An overlay already open is replaced; the keyboard still goes back to
/// where it was before that one opened.
pub fn open_overlay_in(
    frame: Frame,
    window: &mut Window,
    cx: &mut App,
    render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> OverlayHandle {
    let this = window.window_handle();
    let focused = window.focused(cx).map(|f| f.downgrade());
    let focus = cx.focus_handle();
    let overlays = cx.default_global::<Overlays>();
    let previous = match overlays.open.take() {
        Some(replaced) if replaced.window == this => replaced.previous,
        _ => focused,
    };
    overlays.opened += 1;
    let handle = OverlayHandle(overlays.opened);
    overlays.open = Some(Open {
        handle,
        window: this,
        frame,
        focus: focus.clone(),
        previous,
        render: Rc::new(render),
    });
    window.focus(&focus, cx);
    window.refresh();
    handle
}

/// Closes `window`'s overlay, if one is open, and gives the keyboard back
/// to where it was when the overlay opened.
pub fn close_overlay(window: &mut Window, cx: &mut App) {
    let this = window.window_handle();
    let Some(closed) = cx
        .default_global::<Overlays>()
        .open
        .take_if(|open| open.window == this)
    else {
        return;
    };
    if let Some(previous) = closed.previous.and_then(|p| p.upgrade()) {
        window.focus(&previous, cx);
    }
    window.refresh();
}

/// `window`'s open overlay, for [`crate::window::MainWindow`] to draw last.
pub(crate) fn host(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let open = open(cx).filter(|open| open.window == window.window_handle())?;
    let (frame, focus, render) = (open.frame, open.focus.clone(), open.render.clone());
    let content = render(window, cx);
    let theme = cx.theme();
    let backdrop = div()
        .absolute()
        .inset_0()
        .bg(theme.overlay)
        .occlude()
        .on_any_mouse_down(|event, window, cx| {
            // Nothing beneath takes the press; a left click closes.
            cx.stop_propagation();
            if event.button == MouseButton::Left {
                close_overlay(window, cx);
            }
        });
    let surface = v_flex()
        .debug_selector(|| "overlay-frame".into())
        .flex_none()
        .w(px(frame.width))
        .max_w_full()
        .max_h_full()
        .bg(theme.tokens.background)
        .border_1()
        .border_color(theme.border)
        .rounded(px(radius::LG))
        .shadow_xl()
        .overflow_hidden()
        .occlude()
        .child(content);
    // The window less the margin a kit dialog keeps clear. The spacer gives
    // way when the frame would run off the bottom, so the frame moves up.
    let column = v_flex()
        .absolute()
        .inset_0()
        .items_center()
        .p(px(edge::CARD_X))
        .child(div().h(px(frame.top - edge::CARD_X)))
        .child(surface);
    Some(
        div()
            .absolute()
            .inset_0()
            .key_context("Dialog")
            .on_action(|_: &Cancel, window, cx| close_overlay(window, cx))
            .on_action(|_: &Confirm, window, cx| close_overlay(window, cx))
            .child(backdrop)
            .child(column)
            .focus_trap("overlay", &focus)
            .role(Role::Dialog)
            .into_any_element(),
    )
}
