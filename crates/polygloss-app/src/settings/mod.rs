//! Settings: `~/.config/polygloss/settings.json` (design §18), hot-reloaded.
//!
//! [`SettingsStore`] is a GPUI global. It loads the file at startup and
//! reloads it whenever it changes on disk (a `notify` watcher on the config
//! dir). An invalid file keeps the last good settings; the error is kept in
//! [`SettingsStore::last_error`] and shown as a toast by the main window.
//! Observers of the global (`cx.observe_global::<SettingsStore>`) see every
//! change, e.g. review tabs re-apply their viewport options.

pub mod loader;
pub mod model;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{App, AsyncApp, BorrowAppContext as _, Global};
use notify::RecommendedWatcher;
use notify_debouncer_full::{Debouncer, RecommendedCache};

pub use loader::{SETTINGS_FILE, SettingsError};
pub use model::Settings;

use crate::app_state::AppState;

/// The current settings and where they come from (a GPUI global).
pub struct SettingsStore {
    path: PathBuf,
    settings: Arc<Settings>,
    last_error: Option<SettingsError>,
    /// Bumped on every new error (a failed load, even of the same text), so
    /// the window toasts each one once.
    error_generation: u64,
    _watcher: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl Global for SettingsStore {}

/// Loads the settings from the config dir of [`AppState`] and starts
/// watching it. The config dir is created if missing, so a file created
/// later is seen too.
pub fn init(cx: &mut App) {
    let dir = AppState::global(cx).paths.config_dir.clone();
    SettingsStore::init_at(&dir, cx);
}

impl SettingsStore {
    /// [`init`] for the config dir `dir`.
    pub fn init_at(dir: &Path, cx: &mut App) {
        let path = dir.join(SETTINGS_FILE);
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("cannot create {}: {e}", dir.display());
        }
        let (settings, last_error) = match loader::load(&path) {
            Ok(s) => (s, None),
            Err(e) => {
                tracing::warn!("{e}");
                (Settings::default(), Some(e))
            }
        };
        let watcher = match loader::watch(dir) {
            Ok((watcher, rx)) => {
                cx.spawn(async move |cx: &mut AsyncApp| {
                    loop {
                        cx.background_executor().timer(loader::POLL).await;
                        match loader::take_changes(&rx) {
                            Some(true) => cx.update(SettingsStore::reload),
                            Some(false) => {}
                            None => break,
                        }
                    }
                })
                .detach();
                Some(watcher)
            }
            Err(e) => {
                tracing::warn!("not watching {}: {e}", dir.display());
                None
            }
        };
        let error_generation = u64::from(last_error.is_some());
        cx.set_global(SettingsStore {
            path,
            settings: Arc::new(settings),
            last_error,
            error_generation,
            _watcher: watcher,
        });
    }

    pub fn global(cx: &App) -> &SettingsStore {
        cx.global::<SettingsStore>()
    }

    /// The settings in effect (the last good file, or the defaults).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// A cheap handle on the settings in effect.
    pub fn shared(&self) -> Arc<Settings> {
        self.settings.clone()
    }

    /// Why the file on disk is not in effect, if it is not.
    pub fn last_error(&self) -> Option<&SettingsError> {
        self.last_error.as_ref()
    }

    /// Increases with every error reported (see [`Self::last_error`]).
    pub fn error_generation(&self) -> u64 {
        self.error_generation
    }

    /// The settings file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the file again: valid → in effect; invalid → the previous
    /// settings stay and the error is reported.
    pub fn reload(cx: &mut App) {
        let path = SettingsStore::global(cx).path.clone();
        let loaded = loader::load(&path);
        cx.update_global::<SettingsStore, _>(|store, _| match loaded {
            Ok(settings) => {
                store.last_error = None;
                if *store.settings != settings {
                    store.settings = Arc::new(settings);
                }
            }
            Err(e) => {
                tracing::warn!("keeping the previous settings: {e}");
                store.last_error = Some(e);
                store.error_generation += 1;
            }
        });
    }

    /// Puts `settings` in effect without touching the file (the perf
    /// scenarios pin the layout this way). The next change on disk replaces
    /// them.
    pub fn set(settings: Settings, cx: &mut App) {
        cx.update_global::<SettingsStore, _>(|store, _| {
            store.settings = Arc::new(settings);
        });
    }
}
