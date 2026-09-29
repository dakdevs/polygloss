//! `~/.config/polygloss/keymap.json` (design §18): overrides of the default
//! bindings, in a Zed-like shape (our own parser):
//!
//! ```json
//! [{ "context": "Viewport", "bindings": { "j": "viewport::CursorDown", "shift-v": null } }]
//! ```
//!
//! A binding replaces the default of the same keys in the same context;
//! `null` unbinds the keys in that context. `//` comments and trailing commas
//! are allowed, as in `settings.json`. A file that does not parse, names an
//! unknown action, or holds an invalid keystroke or context is rejected as a
//! whole: the previous bindings stay and the window shows the error.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui_kit::{KeyBindingContextPredicate, Keystroke};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use serde::Deserialize;

use crate::keymap::actions;
use crate::settings::model::strip_jsonc;

/// The keymap file's name inside the config dir.
pub const KEYMAP_FILE: &str = "keymap.json";

/// How long the watcher waits for a burst of writes to end.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// One binding of `keymap.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserBinding {
    /// As written (`shift-v`, `cmd-k`, `g g`).
    pub keys: String,
    /// The action's registry name; `None` unbinds the keys.
    pub action: Option<&'static str>,
    /// The section's context; `None` binds everywhere.
    pub context: Option<String>,
}

/// Why `keymap.json` could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for KeymapError {}

#[derive(Deserialize)]
struct Section {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    bindings: BTreeMap<String, Option<String>>,
}

/// Parses a `keymap.json` text. Blank text has no bindings.
pub fn parse(text: &str) -> Result<Vec<UserBinding>, String> {
    let json = strip_jsonc(text);
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let sections: Vec<Section> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for section in sections {
        let context = section
            .context
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty());
        if let Some(c) = &context {
            KeyBindingContextPredicate::parse(c)
                .map_err(|e| format!("invalid context {c:?}: {e}"))?;
        }
        for (keys, action) in section.bindings {
            parse_keys(&keys)?;
            let action = match action {
                None => None,
                Some(name) => Some(
                    actions::find(&name)
                        .ok_or_else(|| format!("unknown action {name:?} for {keys:?}"))?
                        .name,
                ),
            };
            out.push(UserBinding {
                keys,
                action,
                context: context.clone(),
            });
        }
    }
    Ok(out)
}

/// The keystrokes of `keys` (`cmd-k`, `g g`).
pub fn parse_keys(keys: &str) -> Result<Vec<Keystroke>, String> {
    let strokes = keys
        .split_whitespace()
        .map(|k| Keystroke::parse(k).map_err(|_| format!("invalid keystroke {keys:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    if strokes.is_empty() {
        return Err("an empty keystroke".to_owned());
    }
    Ok(strokes)
}

/// Reads and parses the keymap file at `path`. A missing file has no
/// bindings.
pub fn load(path: &Path) -> Result<Vec<UserBinding>, KeymapError> {
    let error = |message: String| KeymapError {
        path: path.to_owned(),
        message,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(error(e.to_string())),
    };
    parse(&text).map_err(error)
}

/// Watches `dir` (not recursively) and sends `()` after changes to the
/// keymap file settle, like `settings::loader::watch`. The watch ends when
/// the returned debouncer is dropped.
pub fn watch(
    dir: &Path,
) -> notify::Result<(
    Debouncer<RecommendedWatcher, RecommendedCache>,
    UnboundedReceiver<()>,
)> {
    let (tx, rx) = unbounded();
    let mut debouncer = new_debouncer(DEBOUNCE, None, KeymapEvents(tx))?;
    debouncer.watch(dir, RecursiveMode::NonRecursive)?;
    Ok((debouncer, rx))
}

struct KeymapEvents(UnboundedSender<()>);

impl notify_debouncer_full::DebounceEventHandler for KeymapEvents {
    fn handle_event(&mut self, result: DebounceEventResult) {
        let touches = match &result {
            Ok(events) => events.iter().any(|e| {
                e.paths
                    .iter()
                    .any(|p| p.file_name().is_some_and(|n| n == KEYMAP_FILE))
            }),
            Err(_) => true,
        };
        if touches {
            let _ = self.0.unbounded_send(());
        }
    }
}
