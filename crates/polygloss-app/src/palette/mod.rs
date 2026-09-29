//! Command palette (⌘K), cheat sheet (`?`) and the view toggles (design
//! §11.4, §11.8).
//!
//! - [`command`]: the palette, every action with its binding.
//! - [`cheat_sheet`]: every binding in effect, by context.
//! - [`view_toggles`]: split/unified (`s`, remembered per diff), hide
//!   whitespace (`w`), word diff by words, characters or off; the toolbar's
//!   controls for them are [`toolbar_items`].

pub mod cheat_sheet;
pub mod command;
pub mod view_toggles;

use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::{IconName, Selectable as _, Sizable as _};
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, MenuItem, ParentElement as _,
    Window,
};
use polygloss_diff::rows::Layout;
use polygloss_diff::word::Granularity;
use polygloss_viewport::{LayoutMode, ViewportEvent};

use crate::keymap::actions::{viewport, window as window_actions};
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

/// The toolbar's view controls (§11.4): Unified | Split, and a menu with the
/// automatic layout, hide whitespace and the word diff granularity.
pub fn toolbar_items(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    let viewport = tab.viewport.read(cx);
    let layout = viewport.effective_layout();
    let opts = viewport.options();
    let auto = opts.layout == LayoutMode::Auto;
    let hide_whitespace = opts.diff.ignore_whitespace;
    let word_diff = opts.word_diff;
    let focus = tab.viewport_focus().clone();

    let layouts = ButtonGroup::new("layout-toggle")
        .outline()
        .xsmall()
        .child(
            Button::new("layout-unified")
                .label("Unified")
                .selected(layout == Layout::Unified),
        )
        .child(
            Button::new("layout-split")
                .label("Split")
                .selected(layout == Layout::Split),
        )
        .on_click(cx.listener(|tab, clicked: &Vec<usize>, _, cx| {
            let choice = if clicked.contains(&1) {
                LayoutMode::Split
            } else {
                LayoutMode::Unified
            };
            view_toggles::set_layout_choice(tab, choice, cx);
        }));

    let options = Button::new("view-options")
        .icon(IconName::Settings2)
        .small()
        .ghost()
        .tooltip("Diff view options")
        .debug_selector(|| "view-options".into())
        .dropdown_menu(move |menu, _, _| {
            menu.action_context(focus.clone())
                .menu_with_check("Automatic layout", auto, Box::new(viewport::LayoutAuto))
                .menu_with_check(
                    "Hide whitespace",
                    hide_whitespace,
                    Box::new(viewport::ToggleWhitespace),
                )
                .separator()
                .menu_with_check(
                    "Word diff",
                    word_diff == Some(Granularity::Word),
                    Box::new(viewport::WordDiffWord),
                )
                .menu_with_check(
                    "Character diff",
                    word_diff == Some(Granularity::Char),
                    Box::new(viewport::WordDiffChar),
                )
                .menu_with_check(
                    "No inline highlights",
                    word_diff.is_none(),
                    Box::new(viewport::WordDiffOff),
                )
        });

    vec![
        gpui_kit::div()
            .debug_selector(|| "layout-toggle".into())
            .child(layouts)
            .into_any_element(),
        options.into_any_element(),
    ]
}
