//! View-state persistence (design §11.12, §7.2 `view_state`).
//!
//! Per `diff_id` a review tab remembers where the reader was: the scroll
//! position as a line (the first line shown below the pinned file header,
//! never pixels, see [`polygloss_viewport::Document::top_line`]), collapsed
//! files, revealed context, the split/unified choice, the file tree's
//! expansion, whether the threads panel shows and unsaved composer text. It
//! is restored when the diff opens again ([`attach`], before the first
//! frame) and saved on change, [`SAVE_DEBOUNCE`] after the last one
//! (trailing), off the main thread; closing the tab or quitting saves what
//! is pending at once.
//!
//! What changed is found by observing the tab's viewport and file tree (every
//! scroll, collapse, reveal or layout change notifies them; showing or hiding
//! the threads panel calls [`changed`] itself) and comparing a
//! fresh [`snapshot`] with the last state written, so nothing is written
//! while nothing changed, and an untouched diff (at the top, nothing
//! collapsed, revealed or chosen) writes nothing at all.
//!
//! A live diff that is not pinned has no `diffs` row, so the store refuses
//! its view state (`NotFound { what: "diff" }`, T1.14): it is then kept for
//! this session only ([`SessionViewStates`]; nothing is pinned just for view
//! state) and written by the first save after the diff is pinned (the next
//! change, or [`save_now`], which T3.10 calls after pinning for a comment).
//!
//! The layout is T3.2's (`palette::view_toggles`): it restores the layout
//! itself and writes `view_state.layout` on every toggle; the whole-state
//! save here reads the tab's current choice, so both writers agree.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Global, Subscription, Task, Window};
use polygloss_core::ids::DiffId;
use polygloss_core::review::{Core, CoreError, ScrollAnchorState, ViewState};
use polygloss_diff::rows::Layout;
use polygloss_viewport::{LayoutMode, ScrollTarget};

use crate::app_state::AppState;
use crate::review_tab::ReviewTab;

/// How long after the last change the view state is saved (design §11.12
/// "debounced"; **Provisional**).
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// The view state of every diff shown this session, by `diff_id`: restores
/// what the store could not keep (an unpinned live diff), and is always at
/// least as new as the store.
#[derive(Default)]
pub struct SessionViewStates(HashMap<String, ViewState>);

impl Global for SessionViewStates {}

/// Nothing app-wide: every tab saves its own state.
pub fn init(_cx: &mut App) {}

/// The per-tab state (a [`ReviewTab`] extension).
struct Persist {
    /// The diff `written` belongs to: the tab's, unless the tab has since
    /// switched diffs (a refresh, an iteration).
    written_for: DiffId,
    /// The state the store has for `written_for` (read at attach, then the
    /// last write); `None` when there is none.
    written: Option<ViewState>,
    /// Unsaved composer text by composer key (T3.10).
    composer: BTreeMap<String, String>,
    /// The pending debounced save.
    timer: Option<Task<()>>,
    writer: Arc<Writer>,
    _subscriptions: Vec<Subscription>,
}

/// Background writes of one tab, in order: the latest state wins.
struct Writer {
    core: Core,
    /// The latest state handed over per diff, not written yet (held only
    /// briefly, so the main thread never waits on a write).
    latest: Mutex<Vec<(DiffId, ViewState)>>,
    /// Held for a whole write, so writes land in order.
    writing: Mutex<()>,
    /// The last write did not reach the store (an unpinned live diff, an
    /// error): write again even when nothing changed since.
    unwritten: AtomicBool,
}

impl Writer {
    /// Writes the latest states handed over, if any.
    fn write(&self) {
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let latest = std::mem::take(&mut *self.latest.lock().unwrap_or_else(|e| e.into_inner()));
        for (diff_id, state) in latest {
            match self.core.save_view_state(&diff_id, &state) {
                Ok(()) => self.unwritten.store(false, Ordering::SeqCst),
                // An unpinned live diff: remembered for the session only.
                Err(CoreError::NotFound { .. }) => self.unwritten.store(true, Ordering::SeqCst),
                Err(e) => {
                    self.unwritten.store(true, Ordering::SeqCst);
                    tracing::warn!("saving the view state of {diff_id}: {e}");
                }
            }
        }
    }
}

/// Restores the diff's view state on a new tab (this session's, else the
/// store's) before its first frame, and saves it on change from now on.
/// Runs after the tree and the view toggles are attached.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    let core = AppState::global(cx).core.clone();
    let diff_id = tab.opened.diff_id.clone();
    // What the store has (compared with before every write), and what to
    // restore: this session's state when there is one (it is never older,
    // and the only one of an unpinned live diff).
    let stored = core.load_view_state(&diff_id).unwrap_or_else(|e| {
        tracing::warn!("reading the view state of {diff_id}: {e}");
        None
    });
    let restored = cx
        .try_global::<SessionViewStates>()
        .and_then(|s| s.0.get(diff_id.as_str()))
        .cloned()
        .or_else(|| stored.clone());
    if let Some(state) = &restored {
        restore(tab, state, cx);
    }
    let mut subscriptions = vec![
        cx.observe(&tab.viewport, |tab: &mut ReviewTab, _, cx| changed(tab, cx)),
        cx.on_release(|tab: &mut ReviewTab, cx: &mut App| {
            if let Some(task) = flush(tab, cx) {
                task.detach();
            }
        }),
        cx.on_app_quit(|tab: &mut ReviewTab, cx| {
            let task = flush(tab, cx);
            async move {
                if let Some(task) = task {
                    task.await;
                }
            }
        }),
    ];
    if let Some(tree) = crate::tree::file_tree(tab) {
        subscriptions.push(cx.observe(tree, |tab: &mut ReviewTab, _, cx| changed(tab, cx)));
    }
    tab.insert_extension(Persist {
        composer: restored
            .as_ref()
            .map(|s| s.composer.clone())
            .unwrap_or_default(),
        written_for: diff_id,
        written: stored,
        timer: None,
        writer: Arc::new(Writer {
            core,
            latest: Mutex::new(Vec::new()),
            writing: Mutex::new(()),
            unwritten: AtomicBool::new(false),
        }),
        _subscriptions: subscriptions,
    });
}

/// Applies `state` to `tab`: collapsed files, revealed context, the tree's
/// expansion, then the scroll position (a saved line lands right below its
/// file's pinned header), and the threads panel (shown at once). Paths the
/// diff does not have are skipped. The layout is restored by the view
/// toggles (T3.2).
pub fn restore(tab: &mut ReviewTab, state: &ViewState, cx: &mut Context<ReviewTab>) {
    let files = tab.viewport.read(cx).document().files().clone();
    let index: HashMap<&str, u32> = files
        .iter()
        .enumerate()
        .rev() // the first file of a path wins
        .map(|(i, f)| (f.display_path(), i as u32))
        .collect();
    let file = |path: &str| index.get(path).copied();
    tab.viewport.update(cx, |v, cx| {
        for idx in state.collapsed.iter().filter_map(|p| file(p)) {
            v.set_collapsed(idx, true, cx);
        }
        for (path, ranges) in &state.expanded {
            if let Some(idx) = file(path) {
                v.set_expansions(idx, ranges, cx);
            }
        }
        if let Some(anchor) = &state.scroll_anchor
            && let Some(idx) = file(&anchor.path)
        {
            v.scroll_to(
                ScrollTarget::Line {
                    file_idx: idx,
                    side: anchor.side,
                    // Stored 1-based (the store's convention).
                    line: anchor.line.saturating_sub(1),
                },
                cx,
            );
        }
    });
    if let (Some(dirs), Some(tree)) = (&state.tree_expanded, crate::tree::file_tree(tab)) {
        tree.update(cx, |t, cx| t.set_expanded_dirs(dirs.iter().cloned(), cx));
    }
    if let Some(shown) = state.threads_panel {
        tab.panes.threads_panel = Some(shown);
    }
}

/// The tab's view state as it is now.
pub fn snapshot(tab: &ReviewTab, cx: &App) -> ViewState {
    let viewport = tab.viewport.read(cx);
    let doc = viewport.document();
    let files = doc.files();
    let path = |idx: u32| files[idx as usize].display_path().to_owned();
    // At the very top, or anywhere above the first card (the header card and
    // the canvas around it), there is nothing to restore: it reopens at the
    // top.
    let top = doc.scroll_top();
    let scroll_anchor = if top <= 0.0 || top < doc.header_top(0) {
        None
    } else {
        doc.top_line().map(|(idx, side, line)| ScrollAnchorState {
            path: path(idx),
            side,
            line: line + 1,
        })
    };
    let layout = match crate::palette::view_toggles::layout_choice(tab) {
        Some(LayoutMode::Split) => Some(Layout::Split),
        Some(LayoutMode::Unified) => Some(Layout::Unified),
        Some(LayoutMode::Auto) | None => None,
    };
    // Every folder expanded is the tree's default: nothing to save.
    let tree_expanded = crate::tree::file_tree(tab)
        .map(|t| t.read(cx))
        .filter(|t| !t.all_dirs_expanded())
        .map(|t| t.expanded_dirs());
    ViewState {
        scroll_anchor,
        collapsed: viewport.collapsed().into_iter().map(path).collect(),
        expanded: viewport
            .expansions()
            .into_iter()
            .map(|(idx, ranges)| (path(idx), ranges))
            .collect(),
        layout,
        tree_expanded,
        threads_panel: tab.panes.threads_panel,
        composer: tab
            .extension::<Persist>()
            .map(|p| p.composer.clone())
            .unwrap_or_default(),
        ..ViewState::default()
    }
}

/// Unsaved composer text for `key` (T3.10's composer keys), as restored or
/// last set.
pub fn composer_text<'a>(tab: &'a ReviewTab, key: &str) -> Option<&'a str> {
    tab.extension::<Persist>()?
        .composer
        .get(key)
        .map(String::as_str)
}

/// Sets (or with `None` clears) the unsaved composer text for `key`; saved
/// with the rest of the view state (debounced).
pub fn set_composer_text(
    tab: &mut ReviewTab,
    key: &str,
    text: Option<String>,
    cx: &mut Context<ReviewTab>,
) {
    let Some(persist) = tab.extension_mut::<Persist>() else {
        return;
    };
    let changed = match text {
        Some(text) => persist.composer.insert(key.to_owned(), text.clone()) != Some(text),
        None => persist.composer.remove(key).is_some(),
    };
    if changed {
        self::changed(tab, cx);
    }
}

/// Something may have changed: save [`SAVE_DEBOUNCE`] after the last change.
pub(crate) fn changed(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(persist) = tab.extension_mut::<Persist>() else {
        return;
    };
    // Replacing the task cancels the previous wait.
    persist.timer = Some(cx.spawn(async move |tab, cx| {
        cx.background_executor().timer(SAVE_DEBOUNCE).await;
        tab.update(cx, save_now).ok();
    }));
}

/// Saves the tab's view state now if it changed since the last write (or
/// that write did not reach the store), off the main thread. Call it after
/// pinning a live diff (T3.10) to write what was kept for the session.
pub fn save_now(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    if let Some(task) = flush(tab, cx) {
        task.detach();
    }
}

/// [`save_now`]'s work: the write's task, when there is one.
fn flush(tab: &mut ReviewTab, cx: &mut App) -> Option<Task<()>> {
    let state = snapshot(tab, cx);
    cx.default_global::<SessionViewStates>()
        .0
        .insert(tab.opened.diff_id.as_str().to_owned(), state.clone());
    let diff_id = tab.opened.diff_id.clone();
    let persist = tab.extension_mut::<Persist>()?;
    persist.timer = None;
    if persist.written_for != diff_id {
        // The tab shows another diff now: its stored state is unknown here,
        // so write unless untouched.
        persist.written_for = diff_id.clone();
        persist.written = None;
    }
    let unwritten = persist.writer.unwritten.load(Ordering::SeqCst);
    let same = match &persist.written {
        Some(written) => *written == state,
        // Untouched: nothing to remember.
        None => state == ViewState::default(),
    };
    if same && !unwritten {
        return None;
    }
    persist.written = Some(state.clone());
    let writer = persist.writer.clone();
    {
        let mut latest = writer.latest.lock().unwrap_or_else(|e| e.into_inner());
        latest.retain(|(id, _)| *id != diff_id);
        latest.push((diff_id, state));
    }
    Some(cx.background_spawn(async move { writer.write() }))
}
