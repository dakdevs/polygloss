//! Open in editor (design §11.13).
//!
//! Owned by T3.16. Created as a stub by T3.1, which already calls
//! [`init`] (from `features::init`), [`attach`] (for every new review tab).

use gpui_kit::{App, Context, Window};

use crate::review_tab::ReviewTab;

/// Registers this feature's actions, bindings and globals.
pub fn init(_cx: &mut App) {}

/// Sets this feature up on a new review tab (subscriptions, per-tab state
/// through [`ReviewTab::insert_extension`]).
pub fn attach(_tab: &mut ReviewTab, _window: &mut Window, _cx: &mut Context<ReviewTab>) {}
