//! Command palette (⌘K), cheat sheet (`?`) and the view toggles (design
//! §11.4, §11.8).
//!
//! - [`command`]: the palette, every action with its binding.
//! - [`cheat_sheet`]: every binding in effect, by context.
//! - [`key_cap`]: how both show a key (`Esc` spelled out).
//! - [`view_toggles`]: split/unified (`s`, remembered per diff), hide
//!   whitespace (`w`), wrap lines, word diff by words, characters or off;
//!   the toolbar's controls for them are [`layout_toggle`] and
//!   [`display_menu`] (design §11.4, T6.8).

pub mod cheat_sheet;
pub mod command;
pub mod key_cap;
pub mod view_toggles;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, Anchor, AnyElement, App, Context, FocusHandle, InteractiveElement as _, IntoElement,
    MenuItem, MouseButton, ParentElement as _, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Toggled, Window, div, px,
};
use polygloss_diff::rows::Layout;
use polygloss_viewport::{LayoutMode, ViewportEvent};

use crate::keymap::actions::window as window_actions;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::window::{MainWindow, MenuKind};

/// Registers the palette, the cheat sheet and the view toggles.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |_: &mut MainWindow, _: &window_actions::CommandPalette, window, cx| {
            command::open(window, cx);
        },
    );
    handlers::on_action(
        cx,
        |_: &mut MainWindow, _: &window_actions::CheatSheet, window, cx| {
            cheat_sheet::open(window, cx);
        },
    );
    view_toggles::init(cx);
    crate::window::add_menu_items(
        MenuKind::View,
        vec![
            MenuItem::separator(),
            MenuItem::action("Command Palette…", window_actions::CommandPalette),
        ],
        cx,
    );
}

/// The layout the toolbar last showed, to repaint it when an automatic
/// layout flips with the width.
struct ShownLayout(Layout);

/// Sets up the view toggles of a new tab.
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    view_toggles::attach(tab, window, cx);
    let shown = tab.viewport.read(cx).effective_layout();
    tab.insert_extension(ShownLayout(shown));
    let viewport = tab.viewport.clone();
    cx.subscribe(&viewport, |tab: &mut ReviewTab, viewport, event, cx| {
        if let ViewportEvent::FrameStats(_) = event {
            let layout = viewport.read(cx).effective_layout();
            if let Some(shown) = tab.extension_mut::<ShownLayout>()
                && shown.0 != layout
            {
                shown.0 = layout;
                cx.notify();
            }
        }
    })
    .detach();
}

/// The toolbar's split | unified toggle (design §11.4, `s`): two icon
/// segments in a track, the layout on screen selected (it follows the
/// automatic layout as the width changes); a click chooses that layout for
/// the diff. Not gpui-kit's `TabBar::segmented`, which animates
/// (ADR-0029).
pub fn layout_toggle(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let shown = tab.viewport.read(cx).effective_layout();
    let theme = cx.theme();
    let segment = |id: &'static str, icon: Lucide, layout: Layout, label: &'static str| {
        let selected = shown == layout;
        let mode = match layout {
            Layout::Split => LayoutMode::Split,
            Layout::Unified => LayoutMode::Unified,
        };
        let icon = Icon::new(icon).small();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.))
            .h(px(22.))
            .rounded(px(5.))
            .map(|segment| {
                if selected {
                    segment
                        .bg(theme.tab_active)
                        .shadow_xs()
                        .text_color(theme.foreground)
                        .child(
                            div()
                                .debug_selector(|| "layout-selected".into())
                                .child(icon),
                        )
                } else {
                    let hover = theme.foreground;
                    segment
                        .text_color(theme.muted_foreground)
                        .hover(move |s| s.text_color(hover))
                        .child(icon)
                }
            })
            .role(Role::Button)
            .aria_label(label)
            .aria_toggled(if selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .on_click(
                cx.listener(move |tab, _, _, cx| view_toggles::set_layout_choice(tab, mode, cx)),
            )
            .tooltip(crate::review_tab::toolbar::tooltip(format!("{label} (s)")))
    };
    let split = segment("layout-split", Lucide::Columns2, Layout::Split, "Split");
    let unified = segment("layout-unified", Lucide::Rows2, Layout::Unified, "Unified");
    h_flex()
        .id("layout-toggle")
        .debug_selector(|| "layout-toggle".into())
        .flex_none()
        // A control: a press on it (a segment or the rim) never moves the
        // window.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(7.))
        .bg(theme.tab_bar_segmented)
        .child(split)
        .child(unified)
        .into_any_element()
}

/// The toolbar's display options menu (design §11.4): its items are
/// `features::display_menu_items`, read when it opens. It opens without
/// motion (ADR-0029).
pub fn display_menu(
    _tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let tab = cx.weak_entity();
    Button::new("view-options")
        .debug_selector(|| "view-options".into())
        .icon(Lucide::SlidersHorizontal)
        .small()
        .ghost()
        .tooltip("Display options")
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
            match tab.upgrade() {
                Some(tab) => tab.update(cx, |tab, cx| {
                    crate::features::display_menu_items(tab, menu, window, cx)
                }),
                None => menu,
            }
        })
        .into_any_element()
}

/// One item of a menu built from data ([`fill_menu`]), so what a menu
/// holds can be read without opening it.
pub enum MenuEntry {
    /// A row that only informs (disabled).
    Note(SharedString),
    /// A row that dispatches `action`; `checked` when it is a toggle.
    Action {
        label: SharedString,
        action: Box<dyn Action>,
        checked: Option<bool>,
    },
    Separator,
}

impl MenuEntry {
    pub fn note(text: impl Into<SharedString>) -> MenuEntry {
        MenuEntry::Note(text.into())
    }

    pub fn action(label: impl Into<SharedString>, action: impl Action) -> MenuEntry {
        MenuEntry::Action {
            label: label.into(),
            action: Box::new(action),
            checked: None,
        }
    }

    pub fn check(label: impl Into<SharedString>, checked: bool, action: impl Action) -> MenuEntry {
        MenuEntry::Action {
            label: label.into(),
            action: Box::new(action),
            checked: Some(checked),
        }
    }
}

impl PartialEq for MenuEntry {
    fn eq(&self, other: &MenuEntry) -> bool {
        match (self, other) {
            (MenuEntry::Note(a), MenuEntry::Note(b)) => a == b,
            (MenuEntry::Separator, MenuEntry::Separator) => true,
            (
                MenuEntry::Action {
                    label: a,
                    action: x,
                    checked: c,
                },
                MenuEntry::Action {
                    label: b,
                    action: y,
                    checked: d,
                },
            ) => a == b && x.partial_eq(y.as_ref()) && c == d,
            _ => false,
        }
    }
}

impl std::fmt::Debug for MenuEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MenuEntry::Note(text) => write!(f, "Note({text:?})"),
            MenuEntry::Separator => write!(f, "Separator"),
            MenuEntry::Action {
                label,
                action,
                checked,
            } => write!(f, "Action({label:?}, {}, {checked:?})", action.name()),
        }
    }
}

/// `menu` with `entries`, their actions dispatched from `focus` (the
/// tab's diff, so they reach the tab).
pub fn fill_menu(menu: PopupMenu, entries: Vec<MenuEntry>, focus: FocusHandle) -> PopupMenu {
    entries
        .into_iter()
        .fold(menu.action_context(focus), |menu, entry| match entry {
            MenuEntry::Note(text) => menu.item(PopupMenuItem::new(text).disabled(true)),
            MenuEntry::Action {
                label,
                action,
                checked: Some(checked),
            } => menu.menu_with_check(label, checked, action),
            MenuEntry::Action {
                label,
                action,
                checked: None,
            } => menu.menu(label, action),
            MenuEntry::Separator => menu.separator(),
        })
}
