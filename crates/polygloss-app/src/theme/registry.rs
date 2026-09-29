//! [`ThemeRegistry`]: the built-in Pierre Light/Dark plus every Zed theme
//! file in `~/.config/polygloss/themes/` (design §11.10, §18), hot-reloaded.
//!
//! A file is a Zed theme family (`{ name, themes: [...] }`) and may hold
//! several themes; each is listed by its own `name`. Files load in file-name
//! order, and a theme named like an earlier one (a built-in included)
//! replaces it, so a user can ship their own "Pierre Dark". A file that does
//! not parse is skipped and reported ([`ThemeRegistry::errors`]; the main
//! window toasts each new error once).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui_kit::{App, AsyncApp, Global, SharedString};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use polygloss_highlight::{Appearance, ZedTheme, load_theme_family, pierre_theme};

use crate::settings::loader::take_changes;

/// The themes directory's name inside the config dir.
pub const THEMES_DIR: &str = "themes";

/// How long the watcher waits for a burst of writes to end.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// A theme file that could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeFileError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for ThemeFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// Every theme the app can show, by name (a GPUI global).
pub struct ThemeRegistry {
    dir: PathBuf,
    themes: BTreeMap<SharedString, Arc<ZedTheme>>,
    errors: Vec<ThemeFileError>,
    /// Messages not yet shown to the user (taken by the main window).
    unannounced: Vec<SharedString>,
    _watcher: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl Global for ThemeRegistry {}

/// What one scan of the themes directory found.
struct Scan {
    themes: BTreeMap<SharedString, Arc<ZedTheme>>,
    errors: Vec<ThemeFileError>,
}

/// The built-ins, then every `*.json` in `dir` in file-name order.
fn scan(dir: &Path) -> Scan {
    let mut themes = BTreeMap::new();
    for appearance in [Appearance::Light, Appearance::Dark] {
        let t = pierre_theme(appearance);
        themes.insert(SharedString::from(t.name.clone()), Arc::new(t.clone()));
    }
    let mut errors = Vec::new();
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| is_theme_file(p))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!("reading {}: {e}", dir.display());
            Vec::new()
        }
    };
    files.sort();
    for path in files {
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| load_theme_family(&text).map_err(|e| e.to_string()));
        match loaded {
            Ok(family) => {
                for t in family.themes {
                    themes.insert(SharedString::from(t.name.clone()), Arc::new(t));
                }
            }
            Err(message) => {
                tracing::warn!("skipping theme file {}: {message}", path.display());
                errors.push(ThemeFileError { path, message });
            }
        }
    }
    Scan { themes, errors }
}

/// `*.json`, not hidden (editors' swap and backup files are).
fn is_theme_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "json")
        && !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        && path.is_file()
}

impl ThemeRegistry {
    /// Loads the themes of `dir` (created if missing, so a file dropped in
    /// later is seen) and starts watching it. Reloads call `on_reload`
    /// (the active theme is re-applied from there).
    pub fn init_at(dir: &Path, cx: &mut App, on_reload: fn(&mut App)) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("cannot create {}: {e}", dir.display());
        }
        let Scan { themes, errors } = scan(dir);
        let unannounced = errors.iter().map(announcement).collect();
        let watcher = match watch(dir) {
            Ok((watcher, rx)) => {
                spawn_reloader(rx, cx, on_reload);
                Some(watcher)
            }
            Err(e) => {
                tracing::warn!("not watching {}: {e}", dir.display());
                None
            }
        };
        cx.set_global(ThemeRegistry {
            dir: dir.to_owned(),
            themes,
            errors,
            unannounced,
            _watcher: watcher,
        });
    }

    pub fn global(cx: &App) -> &ThemeRegistry {
        cx.global::<ThemeRegistry>()
    }

    /// The watched directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every theme's name, sorted.
    pub fn names(&self) -> Vec<SharedString> {
        self.themes.keys().cloned().collect()
    }

    /// The theme called `name`.
    pub fn get(&self, name: &str) -> Option<Arc<ZedTheme>> {
        self.themes.get(name).cloned()
    }

    /// The files skipped by the last scan.
    pub fn errors(&self) -> &[ThemeFileError] {
        &self.errors
    }

    /// Queues `message` for the main window to show.
    pub fn announce(&mut self, message: SharedString) {
        self.unannounced.push(message);
    }

    /// The messages not shown yet (the main window shows them as toasts).
    pub fn take_unannounced(&mut self) -> Vec<SharedString> {
        std::mem::take(&mut self.unannounced)
    }

    /// Scans the directory again. Errors that are new since the last scan
    /// (a file broken now, or broken differently) are queued once.
    pub fn reload(cx: &mut App) {
        let dir = ThemeRegistry::global(cx).dir.clone();
        let Scan { themes, errors } = scan(&dir);
        let registry = cx.global_mut::<ThemeRegistry>();
        for e in &errors {
            if !registry.errors.contains(e) {
                registry.unannounced.push(announcement(e));
            }
        }
        registry.themes = themes;
        registry.errors = errors;
    }
}

/// The toast for a skipped file.
fn announcement(e: &ThemeFileError) -> SharedString {
    format!("Could not load theme {e}").into()
}

/// Reloads the registry, then runs `on_reload`, whenever the directory
/// changes. Like the settings watcher: the watcher's thread wakes the app
/// when the platform allows it, else the app polls
/// (`app_state::WATCHER_POLL`).
fn spawn_reloader(mut rx: UnboundedReceiver<()>, cx: &mut App, on_reload: fn(&mut App)) {
    let reload = move |cx: &mut App| {
        ThemeRegistry::reload(cx);
        on_reload(cx);
    };
    if crate::app_state::watchers_wake_the_app(cx) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            while rx.next().await.is_some() {
                take_changes(&mut rx);
                cx.update(reload);
            }
        })
        .detach();
    } else {
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(crate::app_state::WATCHER_POLL)
                    .await;
                match take_changes(&mut rx) {
                    Some(true) => cx.update(reload),
                    Some(false) => {}
                    None => break,
                }
            }
        })
        .detach();
    }
}

/// Watches `dir` (not recursively) and sends `()` after changes to its
/// `.json` files settle.
pub fn watch(
    dir: &Path,
) -> notify::Result<(
    Debouncer<RecommendedWatcher, RecommendedCache>,
    UnboundedReceiver<()>,
)> {
    let (tx, rx) = unbounded();
    let mut debouncer = new_debouncer(DEBOUNCE, None, ThemeEvents(tx))?;
    debouncer.watch(dir, RecursiveMode::NonRecursive)?;
    Ok((debouncer, rx))
}

/// Forwards debounced events that touch a `.json` file.
struct ThemeEvents(UnboundedSender<()>);

impl notify_debouncer_full::DebounceEventHandler for ThemeEvents {
    fn handle_event(&mut self, result: DebounceEventResult) {
        let touches = match &result {
            Ok(events) => events.iter().any(|e| {
                e.paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|x| x == "json"))
            }),
            Err(_) => true,
        };
        if touches {
            let _ = self.0.unbounded_send(());
        }
    }
}
