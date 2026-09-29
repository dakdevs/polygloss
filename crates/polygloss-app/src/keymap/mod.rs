//! Actions, the keymap file and the default bindings (design §11.9, §18,
//! ADR-0025).
//!
//! [`actions`] declares every action; [`defaults`] is the §11.9 table;
//! [`file`] reads `~/.config/polygloss/keymap.json`. [`KeymapStore`] (a GPUI
//! global) resolves the two ([`resolve`]: a user binding replaces the
//! default of the same keys in the same context, `null` unbinds) and
//! installs the result as GPUI key bindings, marked with [`DEFAULT_META`] /
//! [`USER_META`] so a reload swaps exactly them. The file is hot-reloaded;
//! an invalid file keeps the previous bindings and the main window shows the
//! error. [`handlers`] scopes action handlers to a review tab or the window.

pub mod actions;
pub mod defaults;
pub mod file;
pub mod handlers;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::StreamExt as _;
use gpui_kit::{
    App, AsyncApp, BorrowAppContext as _, DummyKeyboardMapper, Global, KeyBinding,
    KeyBindingContextPredicate, KeyBindingMetaIndex, NoAction, SharedString,
};
use notify::RecommendedWatcher;
use notify_debouncer_full::{Debouncer, RecommendedCache};

pub use file::{KEYMAP_FILE, KeymapError, UserBinding};

use crate::app_state::AppState;

/// Marks the default bindings in GPUI's keymap. A `null` in `keymap.json`
/// ([`USER_META`], stronger) suppresses them; bindings without a meta
/// (gpui-kit's) count as the strongest.
pub const DEFAULT_META: KeyBindingMetaIndex = KeyBindingMetaIndex(2);
/// Marks the bindings of `keymap.json`.
pub const USER_META: KeyBindingMetaIndex = KeyBindingMetaIndex(1);

/// Where a binding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingSource {
    Default,
    User,
}

/// A binding in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// GPUI keystrokes (`shift-e`, `cmd-k`).
    pub keys: String,
    /// The action's registry name.
    pub action: &'static str,
    /// `None` binds everywhere.
    pub context: Option<String>,
    pub source: BindingSource,
}

/// The defaults with `keymap.json` applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolved {
    pub bindings: Vec<Binding>,
    /// `(keys, context)` unbound with `null`.
    pub unbound: Vec<(String, Option<String>)>,
}

impl Resolved {
    /// Every binding in effect for `action`, in table order (unbound keys
    /// excluded).
    pub fn bindings_for(&self, action: &str) -> Vec<&Binding> {
        self.bindings
            .iter()
            .filter(|b| b.action == action)
            .filter(|b| {
                !self
                    .unbound
                    .iter()
                    .any(|(keys, ctx)| ctx.is_none() && same_keys(keys, &b.keys))
            })
            .collect()
    }

    /// The keys shown as `action`'s hint: its first binding.
    pub fn hint_for(&self, action: &str) -> Option<&Binding> {
        self.bindings_for(action).into_iter().next()
    }
}

/// Applies `user` to the default bindings. A user binding replaces the
/// defaults (and earlier user bindings) of the same keys in the same
/// context; `null` removes them and suppresses the keys in that context.
pub fn resolve(user: &[UserBinding]) -> Resolved {
    let mut out = Resolved {
        bindings: defaults::DEFAULT_BINDINGS
            .iter()
            .map(|&(keys, action, context)| Binding {
                keys: keys.to_owned(),
                action,
                context: (!context.is_empty()).then(|| context.to_owned()),
                source: BindingSource::Default,
            })
            .collect(),
        unbound: Vec::new(),
    };
    for u in user {
        let context = u.context.as_deref();
        out.bindings.retain(|b| {
            !(same_keys(&b.keys, &u.keys) && same_context(b.context.as_deref(), context))
        });
        match u.action {
            Some(action) => out.bindings.push(Binding {
                keys: u.keys.clone(),
                action,
                context: u.context.clone(),
                source: BindingSource::User,
            }),
            None => out.unbound.push((u.keys.clone(), u.context.clone())),
        }
    }
    out
}

/// Whether two keystroke strings are the same keys (`E` = `shift-e`).
pub fn same_keys(a: &str, b: &str) -> bool {
    let norm = |k: &str| {
        file::parse_keys(k)
            .map(|ks| ks.iter().map(|k| k.unparse()).collect::<Vec<_>>())
            .ok()
    };
    match (norm(a), norm(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

fn same_context(a: Option<&str>, b: Option<&str>) -> bool {
    let parse = |c: Option<&str>| c.map(|c| KeyBindingContextPredicate::parse(c).ok());
    match (parse(a), parse(b)) {
        (Some(Some(a)), Some(Some(b))) => a == b,
        (None, None) => true,
        _ => a == b,
    }
}

/// Whether `keys` types text in a text field: one printable key, with or
/// without shift (`j`, `?`, `R`).
pub fn is_text_key(keys: &str) -> bool {
    let Ok(strokes) = file::parse_keys(keys) else {
        return false;
    };
    let k = &strokes[0];
    let m = k.modifiers;
    !(m.control || m.alt || m.platform || m.function)
        && (k.key.chars().count() == 1 || k.key == "space")
}

/// The contexts where typing must never trigger actions.
const TEXT_CONTEXTS: [&str; 2] = ["Input", "Composer"];

fn mentions(p: &KeyBindingContextPredicate, name: &str) -> bool {
    use KeyBindingContextPredicate as P;
    match p {
        P::Identifier(id) => id.as_ref() == name,
        P::Equal(k, _) | P::NotEqual(k, _) => k.as_ref() == name,
        P::Not(a) => mentions(a, name),
        P::Descendant(a, b) | P::And(a, b) | P::Or(a, b) => mentions(a, name) || mentions(b, name),
    }
}

/// The GPUI context predicate of a binding: its context, and for a text key
/// (see [`is_text_key`]) `!Input && !Composer` too, so single-letter keys are
/// inactive while typing (ADR-0025). A binding whose context names a text
/// context itself is left alone.
pub fn gpui_predicate(keys: &str, context: Option<&str>) -> Option<KeyBindingContextPredicate> {
    use KeyBindingContextPredicate as P;
    let base = context.and_then(|c| P::parse(c).ok());
    if !is_text_key(keys)
        || base
            .as_ref()
            .is_some_and(|p| TEXT_CONTEXTS.iter().any(|n| mentions(p, n)))
    {
        return base;
    }
    let not = |name: &'static str| P::Not(Box::new(P::Identifier(SharedString::new_static(name))));
    let guard = P::And(Box::new(not("Input")), Box::new(not("Composer")));
    Some(match base {
        Some(base) => P::And(Box::new(base), Box::new(guard)),
        None => guard,
    })
}

/// `resolved` as GPUI key bindings.
pub fn gpui_bindings(resolved: &Resolved) -> Vec<KeyBinding> {
    let load = |keys: &str, action, predicate: Option<KeyBindingContextPredicate>| {
        KeyBinding::load(
            keys,
            action,
            predicate.map(Rc::new),
            false,
            None,
            &DummyKeyboardMapper,
        )
    };
    let mut out = Vec::new();
    for b in &resolved.bindings {
        let Some(info) = actions::find(b.action) else {
            continue;
        };
        let predicate = gpui_predicate(&b.keys, b.context.as_deref());
        match load(&b.keys, (info.build)(), predicate) {
            Ok(binding) => out.push(binding.with_meta(match b.source {
                BindingSource::Default => DEFAULT_META,
                BindingSource::User => USER_META,
            })),
            Err(e) => tracing::warn!("skipping binding {:?}: {e}", b.keys),
        }
    }
    for (keys, context) in &resolved.unbound {
        let predicate = context
            .as_deref()
            .and_then(|c| KeyBindingContextPredicate::parse(c).ok());
        if let Ok(binding) = load(keys, Box::new(NoAction), predicate) {
            out.push(binding.with_meta(USER_META));
        }
    }
    out
}

/// The bindings in effect and the keymap file (a GPUI global).
pub struct KeymapStore {
    path: PathBuf,
    user: Vec<UserBinding>,
    resolved: Resolved,
    last_error: Option<KeymapError>,
    /// Bumped on every new error, so each one is shown once.
    error_generation: u64,
    /// The error generation last shown in the main window.
    shown_generation: u64,
    _watcher: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl Global for KeymapStore {}

/// Loads the default bindings and `keymap.json` from the config dir of
/// [`AppState`], installs them, and watches the file.
pub fn init(cx: &mut App) {
    let dir = AppState::global(cx).paths.config_dir.clone();
    KeymapStore::init_at(&dir, cx);
    // An error at launch is shown once the main window is up.
    cx.spawn(async move |cx: &mut AsyncApp| cx.update(show_error))
        .detach();
    cx.observe_global::<KeymapStore>(show_error).detach();
}

impl KeymapStore {
    /// [`init`] for the config dir `dir`.
    pub fn init_at(dir: &Path, cx: &mut App) {
        let path = dir.join(KEYMAP_FILE);
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("cannot create {}: {e}", dir.display());
        }
        let (user, last_error) = match file::load(&path) {
            Ok(user) => (user, None),
            Err(e) => {
                tracing::warn!("{e}");
                (Vec::new(), Some(e))
            }
        };
        let watcher = match file::watch(dir) {
            Ok((watcher, mut rx)) => {
                if crate::app_state::watchers_wake_the_app(cx) {
                    cx.spawn(async move |cx: &mut AsyncApp| {
                        while rx.next().await.is_some() {
                            crate::settings::loader::take_changes(&mut rx);
                            cx.update(KeymapStore::reload);
                        }
                    })
                    .detach();
                } else {
                    cx.spawn(async move |cx: &mut AsyncApp| {
                        loop {
                            let poll = crate::app_state::WATCHER_POLL;
                            cx.background_executor().timer(poll).await;
                            match crate::settings::loader::take_changes(&mut rx) {
                                Some(true) => cx.update(KeymapStore::reload),
                                Some(false) => {}
                                None => break,
                            }
                        }
                    })
                    .detach();
                }
                Some(watcher)
            }
            Err(e) => {
                tracing::warn!("not watching {}: {e}", dir.display());
                None
            }
        };
        let resolved = resolve(&user);
        install(&resolved, cx);
        cx.set_global(KeymapStore {
            path,
            user,
            resolved,
            error_generation: u64::from(last_error.is_some()),
            last_error,
            shown_generation: 0,
            _watcher: watcher,
        });
    }

    pub fn global(cx: &App) -> &KeymapStore {
        cx.global::<KeymapStore>()
    }

    /// The bindings in effect.
    pub fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    /// The bindings `keymap.json` adds (the last good file).
    pub fn user_bindings(&self) -> &[UserBinding] {
        &self.user
    }

    /// Why the file on disk is not in effect, if it is not.
    pub fn last_error(&self) -> Option<&KeymapError> {
        self.last_error.as_ref()
    }

    /// The keymap file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the file again: valid → its bindings replace the previous
    /// ones; invalid → the previous bindings stay and the error is shown.
    pub fn reload(cx: &mut App) {
        let path = KeymapStore::global(cx).path.clone();
        match file::load(&path) {
            Ok(user) => {
                let resolved = resolve(&user);
                let changed = resolved != KeymapStore::global(cx).resolved;
                if changed {
                    install(&resolved, cx);
                }
                cx.update_global::<KeymapStore, _>(|store, _| {
                    store.user = user;
                    store.resolved = resolved;
                    store.last_error = None;
                });
            }
            Err(e) => {
                tracing::warn!("keeping the previous key bindings: {e}");
                cx.update_global::<KeymapStore, _>(|store, _| {
                    store.last_error = Some(e);
                    store.error_generation += 1;
                });
            }
        }
    }
}

/// Replaces our bindings in GPUI's keymap with `resolved`, keeping every
/// other binding (gpui-kit's) in place.
fn install(resolved: &Resolved, cx: &mut App) {
    let ours = |b: &KeyBinding| b.meta() == Some(DEFAULT_META) || b.meta() == Some(USER_META);
    let others: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|b| !ours(b))
        .cloned()
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(others.into_iter().chain(gpui_bindings(resolved)));
}

/// Shows a keymap error not shown yet as a toast in the main window.
fn show_error(cx: &mut App) {
    let Some(store) = cx.try_global::<KeymapStore>() else {
        return;
    };
    if store.error_generation == store.shown_generation {
        return;
    }
    let Some(message) = store
        .last_error
        .as_ref()
        .map(|e| format!("Invalid key bindings, keeping the previous ones: {e}"))
    else {
        return;
    };
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    let generation = store.error_generation;
    let shown = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| main.toast_error(message.into(), window, cx))
    });
    if shown.is_ok() {
        cx.update_global::<KeymapStore, _>(|store, _| store.shown_generation = generation);
    }
}
