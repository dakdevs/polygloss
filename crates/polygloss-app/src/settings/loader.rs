//! Reading `settings.json` and watching it for changes (design §18).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use crate::settings::model::Settings;

/// The settings file's name inside the config dir.
pub const SETTINGS_FILE: &str = "settings.json";

/// How long the watcher waits for a burst of writes (an editor's atomic
/// save) to end before reloading.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Why `settings.json` could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for SettingsError {}

/// Reads and parses the settings file at `path`. A missing file is the
/// defaults.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let error = |message: String| SettingsError {
        path: path.to_owned(),
        message,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(error(e.to_string())),
    };
    Settings::parse(&text).map_err(|e| error(e.to_string()))
}

/// How often the app looks for change notices. The watcher runs on its own
/// thread, which may not wake GPUI tasks directly (GPUI's test scheduler
/// forbids it), so the app polls a channel.
pub const POLL: Duration = Duration::from_millis(150);

/// Watches `dir` (not recursively) and sends `()` after changes to the
/// settings file settle. Watching the directory, not the file, sees files
/// created after startup and editors' rename-into-place saves. The watch
/// ends when the returned debouncer is dropped.
pub fn watch(
    dir: &Path,
) -> notify::Result<(
    Debouncer<RecommendedWatcher, RecommendedCache>,
    Receiver<()>,
)> {
    let (tx, rx) = channel();
    let mut debouncer = new_debouncer(DEBOUNCE, None, SettingsEvents(tx))?;
    debouncer.watch(dir, RecursiveMode::NonRecursive)?;
    Ok((debouncer, rx))
}

/// Forwards debounced events that touch the settings file.
struct SettingsEvents(Sender<()>);

impl notify_debouncer_full::DebounceEventHandler for SettingsEvents {
    fn handle_event(&mut self, result: DebounceEventResult) {
        let touches = match &result {
            Ok(events) => events.iter().any(|e| {
                e.paths
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == SETTINGS_FILE))
            }),
            // Lost events: reload to be safe.
            Err(_) => true,
        };
        if touches {
            let _ = self.0.send(());
        }
    }
}

/// Whether change notices arrived since the last call (all are taken);
/// `None` once the watcher is gone.
pub fn take_changes(rx: &Receiver<()>) -> Option<bool> {
    let mut changed = false;
    loop {
        match rx.try_recv() {
            Ok(()) => changed = true,
            Err(TryRecvError::Empty) => return Some(changed),
            Err(TryRecvError::Disconnected) => return changed.then_some(true),
        }
    }
}
