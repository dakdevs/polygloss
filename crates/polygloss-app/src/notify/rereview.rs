//! The re-review notification (design §17): posted when an agent asks for a
//! re-review while the app is in the background, and its click.

use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, AsyncApp, SharedString, SystemNotification, SystemNotificationResponse,
    TaskExt as _,
};
use polygloss_core::review::{CoreError, Rereview, ReviewSummary};

use super::Notifier;
use crate::app_state::AppState;
use crate::home::row;
use crate::settings::SettingsStore;

const TAG_PREFIX: &str = "rereview:";

/// The notification tag of `review_id`'s re-review request: a newer
/// request for the same review replaces the older notification.
pub fn rereview_tag(review_id: &str) -> String {
    format!("{TAG_PREFIX}{review_id}")
}

/// The review a [`rereview_tag`] names.
pub fn review_of_tag(tag: &str) -> Option<&str> {
    tag.strip_prefix(TAG_PREFIX).filter(|id| !id.is_empty())
}

/// The notifier when a notification may be posted now: bundled, the app
/// unfocused and notifications enabled (the review's own mute is checked
/// once it is read).
fn allowed(cx: &mut App) -> Option<Rc<dyn Notifier>> {
    let notifier = super::notifier(cx);
    let enabled = SettingsStore::global(cx).settings().notifications.enabled;
    (notifier.bundled() && enabled && cx.active_window().is_none()).then_some(notifier)
}

/// An agent asked for a re-review of `review_id`: posts the notification
/// when [`allowed`] and the review is not muted. Also the entry point for
/// a hidden launch that must deliver it (M4).
pub fn rereview_requested(review_id: String, cx: &mut App) {
    if allowed(cx).is_none() {
        return;
    }
    let Some(state) = cx.try_global::<AppState>() else {
        return;
    };
    let core = state.core.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let read = cx
            .background_spawn(async move {
                let summary = core.review_summary(&review_id)?;
                let rereview = core.review_activity(&review_id)?.rereview;
                Ok::<_, CoreError>(summary.zip(rereview))
            })
            .await;
        cx.update(|cx| match read {
            Ok(Some((summary, rereview))) if !summary.muted => {
                // Checked again: the app may have come forward meanwhile.
                if let Some(notifier) = allowed(cx) {
                    notifier.show(notification(&summary, &rereview), cx);
                }
            }
            // Muted, gone, or no longer awaiting a re-review.
            Ok(_) => {}
            Err(e) => tracing::warn!("reading a re-review request: {e}"),
        });
    })
    .detach();
}

/// "claude-code requested a re-review" / "repo · title" and the summary's
/// first block as plain text.
pub fn notification(summary: &ReviewSummary, rereview: &Rereview) -> SystemNotification {
    let who = rereview.requested_by.as_deref().unwrap_or("The agent");
    let what = format!("{} · {}", summary.repo_display, row::title(summary, None));
    let body = match crate::feed::summary_line(&rereview.summary) {
        Some(line) => format!("{what}\n{line}"),
        None => what,
    };
    SystemNotification {
        tag: rereview_tag(&summary.review_id).into(),
        title: format!("{who} requested a re-review").into(),
        body: body.into(),
        actions: Vec::new(),
    }
}

/// A click on one of our notifications.
pub(super) fn on_response(response: SystemNotificationResponse, cx: &mut App) {
    if let Some(review_id) = review_of_tag(&response.tag) {
        focus_review(review_id.to_owned(), cx);
    }
}

/// Brings the app and the main window forward (reopening it when it was
/// closed) with `review_id`'s tab active, opening the review when it has
/// no tab.
pub fn focus_review(review_id: String, cx: &mut App) {
    cx.activate(true);
    crate::window::reopen(cx);
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    let ix = main.read(cx).tabs().find_review(&review_id, cx);
    handle
        .update(cx, |_, window, cx| {
            window.activate_window();
            if let Some(ix) = ix {
                main.update(cx, |main, cx| main.activate_tab(ix, window, cx));
            }
        })
        .ok();
    if ix.is_some() {
        return;
    }
    let Some(state) = cx.try_global::<AppState>() else {
        return;
    };
    let core = state.core.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let id = review_id.clone();
        let summary = cx
            .background_spawn(async move { core.review_summary(&id) })
            .await;
        cx.update(|cx| {
            let Some((handle, main)) = crate::window::main_window(cx) else {
                return;
            };
            handle
                .update(cx, |_, window, cx| {
                    let request = match summary {
                        Ok(Some(summary)) => row::open_request(&summary).ok_or_else(|| {
                            format!("Cannot open this review: unknown key {:?}", summary.key)
                        }),
                        Ok(None) => Err("This review no longer exists".to_owned()),
                        Err(e) => Err(format!("Could not open the review: {e}")),
                    };
                    match request {
                        Ok(req) => {
                            crate::review_tab::open_review(req, window, cx).detach_and_log_err(cx)
                        }
                        Err(message) => {
                            tracing::warn!("{message}");
                            let message = SharedString::from(message);
                            main.update(cx, |main, cx| main.toast_error(message, window, cx));
                        }
                    }
                })
                .ok();
        });
    })
    .detach();
}
