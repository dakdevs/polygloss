//! A popup menu opened from the keyboard (T5.6): gpui-kit's dropdown
//! buttons open only on a click, so an action builds the same menu and shows
//! it under its button. The menu takes the keyboard (arrows, `⏎`, `Esc`) and
//! hands it back to what had it when it closes (an item chosen, `Esc`, a
//! press outside).
//!
//! The owner keeps the open menu in an `Option<KeyMenu>` and renders
//! [`KeyMenu::element`] inside the button's wrapper (made `relative()`), so
//! the menu hangs from the wrapper's bottom edge.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{
    Anchor, AnyElement, App, Context, DismissEvent, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _, Subscription,
    Window, anchored, deferred, div, px,
};

/// An open keyboard menu.
pub struct KeyMenu {
    view: Entity<PopupMenu>,
    /// What had the keyboard before the menu took it.
    previous: Option<FocusHandle>,
    _dismiss: Subscription,
}

impl KeyMenu {
    /// Builds the menu with `build`, focuses it and returns it; `slot`
    /// finds where `this` keeps it, so a dismissed menu is dropped (and the
    /// keyboard handed back). `replacing` is the menu already open in that
    /// slot, if any (the key pressed again while it shows): the new menu
    /// hands the keyboard back to what had it before that one, not to the
    /// replaced menu.
    pub fn open<V: 'static>(
        slot: fn(&mut V) -> Option<&mut Option<KeyMenu>>,
        replacing: Option<KeyMenu>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> KeyMenu {
        let previous = match replacing {
            Some(old) => old.previous,
            None => window.focused(cx),
        };
        let view = PopupMenu::build(window, cx, build);
        let dismiss = cx.subscribe_in(
            &view,
            window,
            move |this, _, _: &DismissEvent, window, cx| {
                if let Some(menu) = slot(this).and_then(Option::take) {
                    menu.restore_focus(window, cx);
                }
                cx.notify();
            },
        );
        window.focus(&view.focus_handle(cx), cx);
        KeyMenu {
            view,
            previous,
            _dismiss: dismiss,
        }
    }

    /// The menu (tests pick its items).
    pub fn view(&self) -> &Entity<PopupMenu> {
        &self.view
    }

    /// Hands the keyboard back when the menu still has it.
    fn restore_focus(&self, window: &mut Window, cx: &mut App) {
        let Some(previous) = &self.previous else {
            return;
        };
        if window.focused(cx).is_none() || self.view.focus_handle(cx).contains_focused(window, cx) {
            window.focus(previous, cx);
        }
    }

    /// The menu hanging from the bottom edge of its button's wrapper, at its
    /// left (`Anchor::TopLeft`) or right (`Anchor::TopRight`) corner. The
    /// wrapper must be `relative()`.
    pub fn element(&self, anchor: Anchor) -> AnyElement {
        let menu = deferred(
            anchored()
                .anchor(anchor)
                .snap_to_window_with_margin(px(8.))
                .child(div().mt_1().child(self.view.clone())),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY);
        let corner = div().absolute().top_full();
        match anchor {
            Anchor::TopRight => corner.right_0(),
            _ => corner.left_0(),
        }
        .debug_selector(|| "key-menu".into())
        .child(menu)
        .into_any_element()
    }
}
