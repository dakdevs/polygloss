//! Reading `settings.json` and watching it for changes (design §18).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::channel::mpsc::{TryRecvError, UnboundedReceiver, UnboundedSender, unbounded};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

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

/// Watches `dir` (not recursively) and sends `()` after changes to the
/// settings file settle. Watching the directory, not the file, sees files
/// created after startup and editors' rename-into-place saves. The watch
/// ends, and the channel closes, when the returned debouncer is dropped.
/// The sends come from the watcher's thread and wake a task awaiting the
/// receiver (see [`crate::app_state::WatchersWakeTheApp`]).
pub fn watch(
    dir: &Path,
) -> notify::Result<(
    Debouncer<RecommendedWatcher, RecommendedCache>,
    UnboundedReceiver<()>,
)> {
    let (tx, rx) = unbounded();
    let mut debouncer = new_debouncer(DEBOUNCE, None, SettingsEvents(tx))?;
    debouncer.watch(dir, RecursiveMode::NonRecursive)?;
    Ok((debouncer, rx))
}

/// Forwards debounced events that touch the settings file.
struct SettingsEvents(UnboundedSender<()>);

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
            let _ = self.0.unbounded_send(());
        }
    }
}

/// Whether change notices arrived since the last call (all are taken);
/// `None` once the watcher is gone. Never registers a waker, so polling
/// with it is safe under GPUI's test scheduler.
pub fn take_changes(rx: &mut UnboundedReceiver<()>) -> Option<bool> {
    let mut changed = false;
    loop {
        match rx.try_recv() {
            Ok(()) => changed = true,
            Err(TryRecvError::Empty) => return Some(changed),
            Err(TryRecvError::Closed) => return changed.then_some(true),
        }
    }
}
