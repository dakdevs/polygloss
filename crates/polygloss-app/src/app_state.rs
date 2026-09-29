//! [`AppState`]: the process-wide core handle every feature reads.

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
