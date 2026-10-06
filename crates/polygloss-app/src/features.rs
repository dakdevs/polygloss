//! Wires every feature module into the app, so feature tasks only edit their
//! own modules (plan M3 "App module map"): their `init`, their `attach` on
//! each review tab, and their toolbar slots (design §11.4, T6.8).

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{AnyElement, App, Context, Window};

use crate::keymap::actions::tab as tab_actions;
use crate::palette::MenuEntry;
use crate::review_tab::ReviewTab;
use crate::review_tab::toolbar::{self, Narrow};

/// Runs every module's `init` (actions, bindings, globals, menu items).
/// `startup::init` has set up [`crate::app_state::AppState`], the settings
/// and gpui-kit before, and builds the menu bar after.
pub fn init(cx: &mut App) {
    crate::theme::init(cx);
    crate::motion::init(cx);
    crate::reduce_motion::init(cx);
    // Before `window`: "Check for Updates…" leads the Polygloss menu.
    crate::updates::init(cx);
    crate::window::init(cx);
    crate::tabs::init(cx);
    crate::keymap::init(cx);
    crate::keyboard::init(cx);
    crate::palette::init(cx);
    crate::home::init(cx);
    crate::open_flow::init(cx);
    crate::review_tab::init(cx);
    crate::tree::init(cx);
    crate::categories::init(cx);
    crate::viewed::init(cx);
    crate::cursor::init(cx);
    crate::markdown::init(cx);
    crate::threads::init(cx);
    crate::composer::init(cx);
    crate::submit::init(cx);
    crate::live::init(cx);
    crate::iterations::init(cx);
    crate::feed::init(cx);
    crate::view_state::init(cx);
    crate::find::init(cx);
    crate::editor::init(cx);
    crate::install_cli::init(cx);
    crate::notify::init(cx);
    crate::ipc::init(cx);
    crate::urls::init(cx);
}

/// Sets every feature up on a new review tab, in toolbar order. Categories
/// partition the files after threads (whose first load may open a section)
/// and before view state (which restores the open sections, then the
/// anchor).
pub fn attach_review_tab(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    crate::tree::attach(tab, window, cx);
    crate::viewed::attach(tab, window, cx);
    crate::cursor::attach(tab, window, cx);
    crate::threads::attach(tab, window, cx);
    crate::categories::attach(tab, window, cx);
    crate::composer::attach(tab, window, cx);
    crate::submit::attach(tab, window, cx);
    crate::live::attach(tab, window, cx);
    crate::iterations::attach(tab, window, cx);
    crate::palette::attach(tab, window, cx);
    crate::feed::attach(tab, window, cx);
    crate::view_state::attach(tab, window, cx);
    crate::find::attach(tab, window, cx);
    crate::editor::attach(tab, window, cx);
}

/// The toolbar's left side (design §11.4): the repo block, the pills of the
/// review's kind, the Live pill, then the iteration pill (Snapshot is in the
/// live header card since T6.13).
pub fn toolbar_left(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    let mut items = vec![toolbar::repo_block(tab, cx)];
    items.extend(toolbar::kind_pills(tab, cx));
    items.extend(crate::live::toolbar_left(tab, window, cx));
    items.extend(crate::iterations::toolbar_left(tab, window, cx));
    items
}

/// The menus [`toolbar_left`]'s items hang outside the row (`i`'s iteration
/// menu while it is open), for when the row is too narrow for those items:
/// they then hang from the left side's bottom-left corner, under the repo
/// block.
pub fn toolbar_left_menus(tab: &ReviewTab) -> Option<AnyElement> {
    crate::iterations::key_menu(tab)
}

/// The toolbar's right side (design §11.4): Find, the threads button,
/// `N/M`, the split | unified toggle, the display options menu and Submit
/// review.
pub fn toolbar_right(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    let mut items: Vec<AnyElement> = toolbar::find_button(tab).into_iter().collect();
    items.push(crate::threads::toolbar_button(tab, window, cx));
    items.extend(crate::viewed::progress_item(tab, window, cx));
    items.push(crate::palette::layout_toggle(tab, window, cx));
    items.push(crate::palette::display_menu(tab, window, cx));
    items.push(crate::submit::button(tab, window, cx));
    items
}

/// The display options menu's items (design §11.4): `N/M` and Find when
/// the toolbar is too narrow for them, the view toggles with Wrap lines,
/// then hiding or showing agent notes.
pub fn display_menu_entries(tab: &ReviewTab, cx: &App) -> Vec<MenuEntry> {
    let narrow = toolbar::narrow(tab);
    let mut entries = Vec::new();
    if narrow >= Narrow::ProgressInMenu {
        entries.extend(crate::viewed::progress_entry(tab));
    }
    if narrow >= Narrow::FindInMenu {
        entries.push(MenuEntry::action("Find in all files", tab_actions::Find));
    }
    if !entries.is_empty() {
        entries.push(MenuEntry::Separator);
    }
    entries.extend(crate::palette::view_toggles::menu_entries(tab, cx));
    if let Some(notes) = crate::threads::agent_notes_entry(tab, cx) {
        entries.push(MenuEntry::Separator);
        entries.push(notes);
    }
    entries
}

/// [`display_menu_entries`] in `menu`, acting on the tab's diff.
pub fn display_menu_items(
    tab: &ReviewTab,
    menu: PopupMenu,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> PopupMenu {
    let entries = display_menu_entries(tab, cx);
    crate::palette::fill_menu(menu, entries, tab.viewport_focus().clone())
}
