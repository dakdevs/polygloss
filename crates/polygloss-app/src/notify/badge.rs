//! The Dock badge (design §17): how many reviews await you.

use gpui_kit::{App, AppContext as _, AsyncApp};
use polygloss_core::store::events::EventKind;

use super::NotifyState;
use crate::app_state::AppState;

/// Whether an event of `kind` can change what awaits you: everything but
/// the app's own drafts and Viewed marks.
pub fn affects_badge(kind: EventKind) -> bool {
    !matches!(kind, EventKind::DraftChanged | EventKind::ViewedChanged)
}

/// The count the Dock badge shows (0 = none).
pub fn badge_count(cx: &App) -> u32 {
    cx.try_global::<NotifyState>().map_or(0, |s| s.badge)
}

/// Re-reads the number of reviews awaiting you on the background executor
/// and updates the badge when it changed.
pub fn refresh(cx: &mut App) {
    let Some(state) = cx.try_global::<AppState>() else {
        return;
    };
    let core = state.core.clone();
    super::notifier(cx);
    let task = cx.spawn(async move |cx: &mut AsyncApp| {
        let count = cx
            .background_spawn(async move { core.awaiting_you_count() })
            .await;
        cx.update(|cx| match count {
            Ok(count) => apply(count, cx),
            Err(e) => tracing::warn!("counting the reviews awaiting you: {e}"),
        });
    });
    cx.global_mut::<NotifyState>().badge_read = Some(task);
}

/// Shows `count` on the Dock badge unless it already does.
fn apply(count: u32, cx: &mut App) {
    let state = cx.global_mut::<NotifyState>();
    if state.badge == count {
        return;
    }
    state.badge = count;
    let notifier = state.notifier.clone();
    notifier.set_badge(polygloss_platform::dock::badge_label(count).as_deref());
}
