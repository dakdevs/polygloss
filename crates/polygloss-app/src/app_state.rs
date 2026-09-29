//! [`AppState`]: the process-wide core handle every feature reads.

use std::time::Duration;

use gpui_kit::{App, Global};
use polygloss_core::paths::DataPaths;
use polygloss_core::review::Core;

/// The shared core (store, snapshots) and the data paths, set once at
/// startup (a GPUI global).
#[derive(Clone)]
pub struct AppState {
    pub core: Core,
    pub paths: DataPaths,
}

impl Global for AppState {}

impl AppState {
    /// Installs the global for `core` (its paths included).
    pub fn set(core: Core, cx: &mut App) {
        let paths = core.paths.clone();
        cx.set_global(AppState { core, paths });
    }

    pub fn global(cx: &App) -> &AppState {
        cx.global::<AppState>()
    }
}

/// Set by [`crate::startup::run`] (the real platform): file watchers wake
/// the app from their own threads as soon as something changes, so an idle
/// app never wakes up. Without it (tests that call `startup::init` under
/// GPUI's test scheduler, which panics when a foreign thread wakes a task)
/// watchers poll their channels every [`WATCHER_POLL`] instead.
pub struct WatchersWakeTheApp;

impl Global for WatchersWakeTheApp {}

/// How often watchers poll their channels without [`WatchersWakeTheApp`].
pub const WATCHER_POLL: Duration = Duration::from_millis(150);

/// Whether watchers may wake the app from their threads
/// ([`WatchersWakeTheApp`]).
pub fn watchers_wake_the_app(cx: &App) -> bool {
    cx.has_global::<WatchersWakeTheApp>()
}
