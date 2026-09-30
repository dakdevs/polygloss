//! Updates through Sparkle (design §18, §21, OQ-16, plan T5.3).
//!
//! [`init`] starts the updater (`polygloss_platform::sparkle`) once. Only a
//! release bundle built with an appcast (`POLYGLOSS_APPCAST_URL` and
//! `SPARKLE_PUBLIC_ED_KEY` at package time) has one; dev builds, tests and
//! bundles without an appcast have none. With an updater:
//!
//! - **Polygloss › Check for Updates…** (`window::CheckForUpdates`, also in
//!   the command palette) runs Sparkle's user-initiated check. Without one,
//!   neither the menu item nor the palette row exists, and the action (if
//!   bound in `keymap.json`) does nothing.
//! - **`updates.automatic_checks`**: `null` (the default) leaves automatic
//!   checks to Sparkle, which asks on the second launch (the bundle leaves
//!   `SUEnableAutomaticChecks` unset, so nothing is checked before the user
//!   agrees). `true`/`false` overrides that answer, at launch and whenever
//!   the setting changes; going back to `null` keeps Sparkle's current state.
//!
//! In test mode (`POLYGLOSS_TEST=1`) the updater is only loaded, never
//! started ([`StartMode::Idle`]): the bundle smoke test sees it in
//! `debug_state` (`updater: "idle"`) without any update check or prompt.
//!
//! The platform side is an [`UpdateBackend`] (in a GPUI global); tests
//! inject their own with [`set_backend`] before [`init`].

use std::rc::Rc;

use gpui_kit::{App, Global, MenuItem};
use polygloss_platform::sparkle::{StartMode, Updater};

use crate::keymap::actions::window::CheckForUpdates;
use crate::settings::SettingsStore;
use crate::window::{MenuKind, add_menu_items};

/// The registry name of the action, which the palette hides without an
/// updater.
pub const ACTION_NAME: &str = "window::CheckForUpdates";

/// What the app asks of the updater.
pub trait UpdateBackend {
    /// Sparkle's user-initiated check ("Check for Updates…").
    fn check_for_updates(&self);
    /// Turns automatic checks on or off (`updates.automatic_checks`).
    fn set_automatic_checks(&self, on: bool);
    /// `"started"`, or `"idle"` when loaded without starting (test mode);
    /// `debug_state` reports it.
    fn state(&self) -> &'static str;
}

/// The real updater: Sparkle.
struct Sparkle(polygloss_platform::sparkle::Updater);

impl UpdateBackend for Sparkle {
    fn check_for_updates(&self) {
        self.0.check_for_updates();
    }

    fn set_automatic_checks(&self, on: bool) {
        self.0.set_automatically_checks(on);
    }

    fn state(&self) -> &'static str {
        match self.0.mode() {
            StartMode::Start => "started",
            StartMode::Idle => "idle",
        }
    }
}

/// The app's updater (a GPUI global), if any.
struct Updates {
    backend: Option<Rc<dyn UpdateBackend>>,
    /// The `updates.automatic_checks` value last applied.
    applied: Option<bool>,
}

impl Global for Updates {}

/// Replaces the updater (`None`: no updater). Call before [`init`], which
/// otherwise starts Sparkle.
pub fn set_backend(backend: Option<Rc<dyn UpdateBackend>>, cx: &mut App) {
    cx.set_global(Updates {
        backend,
        applied: None,
    });
}

/// Starts Sparkle, or `None` (logged) when this build has no updater.
fn start_sparkle() -> Option<Rc<dyn UpdateBackend>> {
    let mode = if std::env::var_os(crate::perf::TEST_ENV).is_some_and(|v| v == "1") {
        StartMode::Idle
    } else {
        StartMode::Start
    };
    match Updater::try_start(mode) {
        Ok(updater) => {
            tracing::info!("Sparkle updater {:?}", updater.mode());
            Some(Rc::new(Sparkle(updater)))
        }
        Err(why) => {
            tracing::info!("no updater: {why}");
            None
        }
    }
}

/// Starts the updater and, when there is one, adds "Check for Updates…" to
/// the Polygloss menu (before Settings…) and follows
/// `updates.automatic_checks`. Runs before `window::init`, so the item comes
/// first in the menu.
pub fn init(cx: &mut App) {
    if !cx.has_global::<Updates>() {
        let backend = start_sparkle();
        set_backend(backend, cx);
    }
    cx.on_action(|_: &CheckForUpdates, cx| {
        if let Some(backend) = backend(cx) {
            backend.check_for_updates();
        }
    });
    if !available(cx) {
        return;
    }
    add_menu_items(
        MenuKind::App,
        vec![
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
        ],
        cx,
    );
    apply_setting(cx);
    cx.observe_global::<SettingsStore>(apply_setting).detach();
}

fn backend(cx: &App) -> Option<Rc<dyn UpdateBackend>> {
    cx.try_global::<Updates>()?.backend.clone()
}

/// Whether this build has an updater.
pub fn available(cx: &App) -> bool {
    backend(cx).is_some()
}

/// The updater's [`UpdateBackend::state`], or `None` without one.
pub fn state(cx: &App) -> Option<&'static str> {
    backend(cx).map(|b| b.state())
}

/// Hands a set `updates.automatic_checks` that changed to the updater.
fn apply_setting(cx: &mut App) {
    let wanted = SettingsStore::global(cx)
        .settings()
        .updates
        .automatic_checks;
    let Some(on) = wanted else {
        // `null`: Sparkle's prompt (or its last state) decides.
        cx.global_mut::<Updates>().applied = None;
        return;
    };
    let updates = cx.global_mut::<Updates>();
    if updates.applied == Some(on) {
        return;
    }
    updates.applied = Some(on);
    if let Some(backend) = updates.backend.clone() {
        backend.set_automatic_checks(on);
    }
}
