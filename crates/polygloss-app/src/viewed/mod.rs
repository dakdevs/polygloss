//! Viewed UX (design §9).
//!
//! Owned by T3.7. Created as a stub by T3.1, which already calls
//! [`init`] (from `features::init`), [`attach`] (for every new review tab), [`toolbar_items`] (by the review tab's toolbar).

use gpui_kit::{AnyElement, App, Context, Window};

use crate::review_tab::ReviewTab;

/// Registers this feature's actions, bindings and globals.
pub fn init(_cx: &mut App) {}

/// Sets this feature up on a new review tab (subscriptions, per-tab state
/// through [`ReviewTab::insert_extension`]).
pub fn attach(_tab: &mut ReviewTab, _window: &mut Window, _cx: &mut Context<ReviewTab>) {}

/// This feature's toolbar controls, in toolbar order (design §11.4).
pub fn toolbar_items(
    _tab: &ReviewTab,
    _window: &mut Window,
    _cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    Vec::new()
}
