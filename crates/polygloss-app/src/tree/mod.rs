//! File tree and file finder (⌘P) (design §11.5).
//!
//! Owned by T3.6. Created as a stub by T3.1, which already calls
//! [`init`] (from `features::init`), [`attach`] (for every new review tab), [`render_pane`] (the review tab's left pane).

use gpui_kit::{AnyElement, App, Context, Window};

use crate::review_tab::ReviewTab;

/// Registers this feature's actions, bindings and globals.
pub fn init(_cx: &mut App) {}

/// Sets this feature up on a new review tab (subscriptions, per-tab state
/// through [`ReviewTab::insert_extension`]).
pub fn attach(_tab: &mut ReviewTab, _window: &mut Window, _cx: &mut Context<ReviewTab>) {}

/// The file tree pane; `None` shows the shell's plain file list.
pub fn render_pane(
    _tab: &ReviewTab,
    _window: &mut Window,
    _cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    None
}
