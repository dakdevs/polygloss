//! Wires every feature module into the app, so feature tasks only edit their
//! own modules (plan M3 "App module map"). Edited only by T3.1.

use gpui_kit::{App, Context, Window};

use crate::review_tab::ReviewTab;

/// Runs every module's `init` (actions, bindings, globals, menu items).
/// `startup::init` has set up [`crate::app_state::AppState`], the settings
/// and gpui-kit before, and builds the menu bar after.
pub fn init(cx: &mut App) {
    crate::theme::init(cx);
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
    crate::notify::init(cx);
    crate::ipc::init(cx);
    crate::urls::init(cx);
}

/// Sets every feature up on a new review tab, in toolbar order.
pub fn attach_review_tab(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    crate::tree::attach(tab, window, cx);
    crate::viewed::attach(tab, window, cx);
    crate::cursor::attach(tab, window, cx);
    crate::threads::attach(tab, window, cx);
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
