//! Notifications, Dock badge and mute (design §17, T3.17).
//!
//! - **Notifications** ([`rereview`]): a `review.rereview_requested` event
//!   from the store feed posts a macOS notification through GPUI's
//!   `App::show_system_notification`, but only while the app is unfocused,
//!   `notifications.enabled` is on (the global mute), the review is not
//!   muted (`reviews.muted`) and the app runs from a bundle (outside one
//!   `UNUserNotificationCenter` aborts the process). Plain agent replies,
//!   resolves, notes and questions never notify: their banner is enough.
//!   Clicking the notification focuses the review's tab (opening it when
//!   it is closed); a submission or archive retracts it.
//! - **Dock badge** ([`badge`]): the number of unarchived reviews awaiting
//!   you (re-review requested, or an open agent question without a
//!   published human reply), muted ones included, re-read after every
//!   store batch that can change it.
//!
//! The platform side is a [`Notifier`] (a GPUI global): [`SystemNotifier`]
//! in the app; tests inject their own with [`set_notifier`].

pub mod badge;
pub mod rereview;

use std::rc::Rc;

use gpui_kit::{App, Global, SystemNotification, Task};
use polygloss_core::store::events::{Event, EventKind};

use crate::app_state::AppState;

pub use badge::badge_count;
pub use rereview::{focus_review, rereview_requested, rereview_tag, review_of_tag};

/// The app's bundle identifier (packaging `CFBundleIdentifier`).
pub const BUNDLE_ID: &str = "dev.dak.polygloss";

/// Where notifications and the Dock badge go.
pub trait Notifier {
    /// Whether system notifications can be posted: the app runs from an app
    /// bundle ([`polygloss_platform::bundle::is_bundled`]), outside test mode.
    fn bundled(&self) -> bool;
    /// Posts `notification` (only called when [`Notifier::bundled`]).
    fn show(&self, notification: SystemNotification, cx: &mut App);
    /// Retracts the notification tagged `tag` (only called when bundled).
    fn dismiss(&self, tag: &str, cx: &mut App);
    /// Sets the Dock badge (`None` clears it).
    fn set_badge(&self, label: Option<&str>);
}

/// The real platform: GPUI's system notifications and the AppKit Dock tile.
pub struct SystemNotifier {
    /// Whether notifications may be posted (see [`SystemNotifier::for_process`]).
    bundled: bool,
}

impl SystemNotifier {
    /// For this process: bundled ([`polygloss_platform::bundle::is_bundled`])
    /// and in test mode when `POLYGLOSS_TEST=1`.
    pub fn new() -> SystemNotifier {
        let test_mode = std::env::var_os(polygloss_core::ipc::TEST_ENV).is_some_and(|v| v == "1");
        SystemNotifier::for_process(polygloss_platform::bundle::is_bundled(), test_mode)
    }

    /// Posts notifications only from a bundle outside test mode: a bundle run
    /// by the E2E suites (`POLYGLOSS_TEST=1`) must never reach the user's
    /// Notification Center, whose authorization prompt and settings `HOME`
    /// does not sandbox (plan T5.7). The Dock badge is unaffected.
    pub fn for_process(bundled: bool, test_mode: bool) -> SystemNotifier {
        SystemNotifier {
            bundled: bundled && !test_mode,
        }
    }
}

impl Default for SystemNotifier {
    fn default() -> SystemNotifier {
        SystemNotifier::new()
    }
}

impl Notifier for SystemNotifier {
    fn bundled(&self) -> bool {
        self.bundled
    }

    fn show(&self, notification: SystemNotification, cx: &mut App) {
        cx.show_system_notification(notification);
    }

    fn dismiss(&self, tag: &str, cx: &mut App) {
        cx.dismiss_system_notification(tag);
    }

    fn set_badge(&self, label: Option<&str>) {
        polygloss_platform::dock::set_badge(label);
    }
}

/// The notifier and the badge as last set (a GPUI global).
struct NotifyState {
    notifier: Rc<dyn Notifier>,
    /// The count the Dock badge shows (0 = no badge, as at launch).
    badge: u32,
    /// The badge read in flight; a newer one replaces (cancels) it.
    badge_read: Option<Task<()>>,
}

impl Global for NotifyState {}

/// Replaces where notifications and the Dock badge go (tests).
pub fn set_notifier(notifier: Rc<dyn Notifier>, cx: &mut App) {
    if cx.has_global::<NotifyState>() {
        cx.global_mut::<NotifyState>().notifier = notifier;
    } else {
        cx.set_global(NotifyState {
            notifier,
            badge: 0,
            badge_read: None,
        });
    }
}

/// The installed notifier.
pub fn notifier(cx: &mut App) -> Rc<dyn Notifier> {
    if !cx.has_global::<NotifyState>() {
        set_notifier(Rc::new(SystemNotifier::new()), cx);
    }
    cx.global::<NotifyState>().notifier.clone()
}

/// Installs the system notifier, handles notification clicks, reacts to
/// store events and sets the Dock badge at launch.
pub fn init(cx: &mut App) {
    if !cx.has_global::<NotifyState>() {
        set_notifier(Rc::new(SystemNotifier::new()), cx);
    }
    // A no-op on macOS (the bundle names the app); GPUI's other platforms
    // and its test platform want it before the first notification.
    cx.set_app_identity(BUNDLE_ID, "Polygloss");
    // Registered in every build: outside a bundle GPUI leaves the
    // notification center alone; inside one this also catches the click
    // that launched the app.
    cx.on_system_notification_response(rereview::on_response);
    crate::feed::on_events(cx, on_events);
    // Reopening an archived review un-archives it without an event.
    crate::review_tab::on_new_tab(cx, |_, _, cx| badge::refresh(cx));
    if cx.has_global::<AppState>() {
        badge::refresh(cx);
    }
}

/// A batch of store events: re-read the badge when it can have changed,
/// notify re-review requests, retract the notification of a review that
/// was submitted or archived.
fn on_events(events: &[Event], cx: &mut App) {
    if events.iter().any(|e| badge::affects_badge(e.kind)) {
        badge::refresh(cx);
    }
    let mut requested: Vec<&str> = Vec::new();
    let mut ended: Vec<&str> = Vec::new();
    for event in events {
        let Some(review_id) = event.review_id.as_deref() else {
            continue;
        };
        match event.kind {
            EventKind::ReviewRereviewRequested => {
                ended.retain(|id| *id != review_id);
                if !requested.contains(&review_id) {
                    requested.push(review_id);
                }
            }
            EventKind::ReviewSubmitted | EventKind::ReviewArchived => {
                requested.retain(|id| *id != review_id);
                if !ended.contains(&review_id) {
                    ended.push(review_id);
                }
            }
            _ => {}
        }
    }
    for review_id in requested {
        rereview_requested(review_id.to_owned(), cx);
    }
    if !ended.is_empty() {
        let notifier = notifier(cx);
        if notifier.bundled() {
            for review_id in ended {
                notifier.dismiss(&rereview_tag(review_id), cx);
            }
        }
    }
}
