//! Viewed UX (design §9, ADR-0022).
//!
//! - **Toggles:** `v` in the viewport (`viewport::ToggleViewed`: the
//!   cursor's file, else the file at the top), `v` in the tree
//!   (`tree::ToggleViewed`: the selected file, or the selected folder), the
//!   header checkbox (`ViewportEvent::ViewedToggled`) and the tree's
//!   checkboxes (`FileTreeEvent`). Marking a file viewed collapses it and
//!   brings the next unviewed shown file in display order (wrapping, never
//!   into a closed category section) to the top with the cursor on its
//!   first line; unmarking expands it again. Marking files of a closed
//!   section never moves the view (design §9).
//! - **Folders** (provisional): a folder's checkbox is tri-state (all, some,
//!   none of its files viewed); clicking it or `v` on it checks every file
//!   below it, or unchecks them all when all are viewed.
//!   `tree::MarkFolderViewed` checks the selected folder (or the selected
//!   file's folder). The viewport jumps only when it was showing one of them.
//! - **State:** the tab keeps each file's [`ViewedState`] from
//!   `Core::viewed_states` (so a path this review viewed with another blob
//!   pair shows "changed since viewed"), loaded on the background executor
//!   when the tab opens (viewed files then start collapsed, as on GitHub)
//!   and again whenever another tab changes Viewed marks. Toggles update it
//!   at once and write `Core::set_viewed` in order on the background
//!   executor; a failed write shows a toast and reloads from the store.
//!   Toggling never pins (the key is the blob pair, design §5.2).
//! - **Flags:** [`update_file_flags`] pushes the same `Vec<FileFlags>` to
//!   the viewport's headers and the tree's rows. This module sets only
//!   `viewed` and `changed_since_viewed`; the thread fields are the
//!   threads feature's (T3.9), which should set them through
//!   [`update_file_flags`] too, so neither overwrites the other.
//! - **Toolbar:** a compact `N/M` with the tooltip "N of M files viewed"
//!   ([`progress_item`], selector `viewed-progress`, T6.8); in the display
//!   options menu once the toolbar is narrow ([`progress_entry`]).
//!
//! Owned by T3.7. `features` calls [`init`], [`attach`] (for every new
//! review tab) and [`progress_item`] (for the toolbar).

use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    div,
};
use polygloss_core::review::ViewedState;
use polygloss_diff::FileChange;
use polygloss_viewport::{FileFlags, ViewportEvent};

use crate::app_state::AppState;
use crate::keymap::actions::{tree as tree_actions, viewport as viewport_actions};
use crate::keymap::handlers;
use crate::live::DiffRefreshed;
use crate::palette::MenuEntry;
use crate::review_tab::ReviewTab;
use crate::review_tab::toolbar::{self, Narrow};
use crate::space::{TextStyleExt as _, text};
use crate::tree::{FileTreeEvent, file_tree};

/// Registers the Viewed actions on review tabs.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport_actions::ToggleViewed, _, cx| {
            let v = tab.viewport.read(cx);
            let idx = v.cursor().map_or(v.anchor().file_idx, |c| c.file_idx);
            toggle_file(tab, idx, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tree_actions::ToggleViewed, _, cx| {
            let Some(tree) = file_tree(tab).cloned() else {
                return;
            };
            let tree = tree.read(cx);
            if let Some(idx) = tree.selected_file() {
                toggle_file(tab, idx, cx);
            } else if let Some((_, files)) = tree.selected_dir() {
                toggle_folder(tab, files, cx);
            } else if let Some(idx) = tree.highlighted_file() {
                toggle_file(tab, idx, cx);
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tree_actions::MarkFolderViewed, _, cx| {
            if let Some(files) = folder_for_action(tab, cx) {
                set_viewed(tab, &files, true, cx);
            }
        },
    );
}

/// Bumped after every Viewed write, so every tab reloads its marks (the
/// key is global: another tab may show the same file change).
#[derive(Default)]
struct ViewedRevision(u64);

impl Global for ViewedRevision {}

/// A tab's Viewed state (a [`ReviewTab`] extension).
struct TabViewed {
    states: Vec<ViewedState>,
    /// The first load from the store finished.
    loaded: bool,
    /// Bumped by every local change: a load started before one is stale.
    generation: u64,
    /// The last store operation queued (loads and writes run in order).
    queue: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Sets Viewed up on a new review tab: handles the header and tree
/// checkboxes and loads the marks.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    if !cx.has_global::<ViewedRevision>() {
        cx.set_global(ViewedRevision::default());
    }
    let mut subscriptions = vec![
        cx.subscribe(
            &tab.viewport,
            |tab: &mut ReviewTab, _, event: &ViewportEvent, cx| {
                if let ViewportEvent::ViewedToggled(idx) = *event {
                    toggle_file(tab, idx, cx);
                }
            },
        ),
        cx.observe_global::<ViewedRevision>(|tab: &mut ReviewTab, cx| reload(tab, cx)),
        // Another diff is shown (a refresh, another iteration): its files'
        // marks, including "changed since viewed", come from the store.
        cx.subscribe_self(|tab: &mut ReviewTab, _: &DiffRefreshed, cx| reload(tab, cx)),
    ];
    if let Some(tree) = file_tree(tab).cloned() {
        subscriptions.push(cx.subscribe(
            &tree,
            |tab: &mut ReviewTab, _, event: &FileTreeEvent, cx| match event {
                FileTreeEvent::ToggleViewed(idx) => toggle_file(tab, *idx, cx),
                FileTreeEvent::ToggleFolderViewed { files, .. } => {
                    toggle_folder(tab, files.clone(), cx)
                }
            },
        ));
    }
    tab.insert_extension(TabViewed {
        states: vec![ViewedState::NotViewed; tab.opened.files.len()],
        loaded: false,
        generation: 0,
        queue: None,
        _subscriptions: subscriptions,
    });
    reload(tab, cx);
}

/// Each file's Viewed state, in file order (`None` before [`attach`]).
pub fn states(tab: &ReviewTab) -> Option<&[ViewedState]> {
    tab.extension::<TabViewed>().map(|v| v.states.as_slice())
}

/// Whether the marks were loaded from the store.
pub fn is_loaded(tab: &ReviewTab) -> bool {
    tab.extension::<TabViewed>().is_some_and(|v| v.loaded)
}

/// `(viewed, total)` files.
pub fn progress(tab: &ReviewTab) -> (usize, usize) {
    let viewed = states(tab).map_or(0, |s| {
        s.iter().filter(|&&s| s == ViewedState::Viewed).count()
    });
    (viewed, tab.opened.files.len())
}

/// The toolbar's text: `N/M`.
pub fn progress_label(tab: &ReviewTab) -> String {
    let (viewed, total) = progress(tab);
    format!("{viewed}/{total}")
}

/// [`progress_label`] in words: "N of M files viewed".
pub fn progress_tooltip(tab: &ReviewTab) -> String {
    match progress(tab) {
        (viewed, 1) => format!("{viewed} of 1 file viewed"),
        (viewed, total) => format!("{viewed} of {total} files viewed"),
    }
}

/// Reloads the marks from the store (after the queued writes). Call it
/// when the tab's files change (a refresh) or the store changed elsewhere.
pub fn reload(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let files: Arc<Vec<FileChange>> = tab.opened.files.clone();
    let Some(ext) = tab.extension_mut::<TabViewed>() else {
        return;
    };
    let generation = ext.generation;
    let previous = ext.queue.take();
    ext.queue = Some(cx.spawn(async move |this, cx| {
        if let Some(previous) = previous {
            previous.await;
        }
        let loaded = cx
            .background_spawn(async move { core.viewed_states(Some(&review_id), &files) })
            .await;
        this.update(cx, |tab, cx| match loaded {
            Ok(states) => apply_loaded(tab, generation, states, cx),
            Err(e) => tracing::warn!("loading the Viewed marks of {}: {e}", tab.review_id),
        })
        .ok();
    }));
}

/// Takes loaded marks, unless a toggle happened meanwhile (then loads
/// again, after its write).
fn apply_loaded(
    tab: &mut ReviewTab,
    generation: u64,
    mut states: Vec<ViewedState>,
    cx: &mut Context<ReviewTab>,
) {
    let files = tab.opened.files.len();
    let Some(ext) = tab.extension_mut::<TabViewed>() else {
        return;
    };
    if ext.generation != generation {
        reload(tab, cx);
        return;
    }
    states.resize(files, ViewedState::NotViewed);
    let first = !ext.loaded;
    ext.loaded = true;
    if ext.states == states && !first {
        return;
    }
    ext.states = states;
    if first {
        // Viewed files open collapsed, as on GitHub.
        let viewed: Vec<u32> = viewed_files(tab);
        tab.viewport.update(cx, |v, cx| {
            for idx in viewed {
                v.set_collapsed(idx, true, cx);
            }
        });
    }
    push_flags(tab, cx);
    cx.notify();
}

fn viewed_files(tab: &ReviewTab) -> Vec<u32> {
    states(tab)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter(|(_, s)| **s == ViewedState::Viewed)
        .map(|(i, _)| i as u32)
        .collect()
}

/// Reads the viewport's file flags, lets `f` change them, and pushes them
/// to the viewport's headers and the tree's rows.
pub fn update_file_flags(tab: &ReviewTab, cx: &mut App, f: impl FnOnce(&mut Vec<FileFlags>)) {
    let mut flags = tab.viewport.read(cx).file_flags().to_vec();
    flags.resize(tab.opened.files.len(), FileFlags::default());
    f(&mut flags);
    if let Some(tree) = file_tree(tab) {
        let flags = flags.clone();
        tree.update(cx, |t, cx| t.set_file_flags(flags, cx));
    }
    tab.viewport.update(cx, |v, cx| v.set_file_flags(flags, cx));
}

/// Pushes the Viewed marks into the flags.
fn push_flags(tab: &ReviewTab, cx: &mut App) {
    let Some(states) = states(tab) else {
        return;
    };
    let states = states.to_vec();
    update_file_flags(tab, cx, |flags| {
        for (flag, state) in flags.iter_mut().zip(&states) {
            flag.viewed = *state == ViewedState::Viewed;
            flag.changed_since_viewed = *state == ViewedState::ChangedSinceViewed;
        }
    });
}

fn is_viewed(tab: &ReviewTab, idx: u32) -> bool {
    states(tab)
        .and_then(|s| s.get(idx as usize))
        .is_some_and(|s| *s == ViewedState::Viewed)
}

/// Toggles file `idx`; marking a shown file viewed jumps to the next
/// unviewed file (marking one in a closed section never moves the view).
pub fn toggle_file(tab: &mut ReviewTab, idx: u32, cx: &mut Context<ReviewTab>) {
    if idx as usize >= tab.opened.files.len() {
        return;
    }
    let viewed = !is_viewed(tab, idx);
    let hidden = tab.viewport.read(cx).is_hidden(idx);
    mark(tab, &[idx], viewed, cx);
    if viewed && !hidden {
        jump_to_next_unviewed(tab, idx, cx);
    }
}

/// Checks every file of a folder, or unchecks them all when all are
/// viewed.
pub fn toggle_folder(tab: &mut ReviewTab, files: Vec<u32>, cx: &mut Context<ReviewTab>) {
    let viewed = !files.iter().all(|&f| is_viewed(tab, f));
    set_viewed(tab, &files, viewed, cx);
}

/// Marks `files` (a folder's or a section's) viewed or not ([`mark`]).
/// Marking them viewed jumps only when they hold the cursor's file (else the
/// anchor's) and it is shown, from the last of them in display order
/// (design §9); marking files in a closed section never moves the view.
pub fn set_viewed(tab: &mut ReviewTab, files: &[u32], viewed: bool, cx: &mut Context<ReviewTab>) {
    let v = tab.viewport.read(cx);
    let current = v.cursor().map_or(v.anchor().file_idx, |c| c.file_idx);
    let jump_from = (viewed && files.contains(&current) && !v.is_hidden(current))
        .then(|| files.iter().copied().max_by_key(|&f| v.display_rank(f)))
        .flatten();
    mark(tab, files, viewed, cx);
    if let Some(last) = jump_from {
        jump_to_next_unviewed(tab, last, cx);
    }
}

/// Marks `files` viewed or not: updates the state and the flags at once,
/// collapses (or expands) them, and queues the store write.
fn mark(tab: &mut ReviewTab, files: &[u32], viewed: bool, cx: &mut Context<ReviewTab>) {
    let target = if viewed {
        ViewedState::Viewed
    } else {
        ViewedState::NotViewed
    };
    let all = tab.opened.files.clone();
    let Some(ext) = tab.extension_mut::<TabViewed>() else {
        return;
    };
    let changed: Vec<u32> = files
        .iter()
        .copied()
        .filter(|&f| {
            ext.states
                .get(f as usize)
                .is_some_and(|s| (*s == ViewedState::Viewed) != viewed)
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    for &f in &changed {
        ext.states[f as usize] = target;
    }
    ext.generation += 1;
    let writes: Vec<FileChange> = changed.iter().map(|&f| all[f as usize].clone()).collect();
    queue_write(tab, writes, viewed, cx);
    push_flags(tab, cx);
    tab.viewport.update(cx, |v, cx| {
        for &f in &changed {
            v.set_collapsed(f, viewed, cx);
        }
    });
    cx.notify();
}

/// Queues `Core::set_viewed` for `changes` after the previous operations.
fn queue_write(
    tab: &mut ReviewTab,
    changes: Vec<FileChange>,
    viewed: bool,
    cx: &mut Context<ReviewTab>,
) {
    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let Some(ext) = tab.extension_mut::<TabViewed>() else {
        return;
    };
    let previous = ext.queue.take();
    ext.queue = Some(cx.spawn(async move |this, cx| {
        if let Some(previous) = previous {
            previous.await;
        }
        let written = cx
            .background_spawn(async move {
                changes
                    .iter()
                    .try_for_each(|c| core.set_viewed(Some(&review_id), c, viewed))
            })
            .await;
        cx.update(|cx| match written {
            Ok(()) => cx.global_mut::<ViewedRevision>().0 += 1,
            Err(e) => {
                let message = format!("Could not save the Viewed mark: {e}");
                tracing::warn!("{message}");
                if let Some(tab) = this.upgrade() {
                    tab.update(cx, reload);
                }
                toast(message, cx);
            }
        });
    }));
}

/// Shows `message` as an error toast in the main window.
fn toast(message: String, cx: &mut App) {
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            main.update(cx, |m, cx| m.toast_error(message.into(), window, cx))
        })
        .ok();
}

/// Brings the first unviewed shown file after `after` in display order
/// (wrapping over shown files; never into a closed section) to the top,
/// with the cursor on it and its row selected in the tree. Nothing when
/// every shown file is viewed.
fn jump_to_next_unviewed(tab: &mut ReviewTab, after: u32, cx: &mut Context<ReviewTab>) {
    let Some(states) = states(tab) else {
        return;
    };
    let v = tab.viewport.read(cx);
    let order = v.display_order();
    let rank = v.display_rank(after) as usize;
    let next = order[rank.min(order.len())..]
        .iter()
        .skip(1)
        .chain(&order[..rank.min(order.len())])
        .copied()
        .find(|&f| {
            !v.is_hidden(f)
                && states
                    .get(f as usize)
                    .is_some_and(|s| *s != ViewedState::Viewed)
        });
    let Some(next) = next else {
        return;
    };
    if let Some(tree) = file_tree(tab).cloned() {
        tree.update(cx, |t, cx| t.select_file(next, cx));
    }
    tab.viewport.update(cx, |v, cx| v.go_to_file(next, cx));
}

/// The files of the folder `tree::MarkFolderViewed` acts on: the selected
/// folder, else the folder of the selected (or highlighted) file.
fn folder_for_action(tab: &ReviewTab, cx: &App) -> Option<Vec<u32>> {
    let tree = file_tree(tab)?.read(cx);
    if let Some((_, files)) = tree.selected_dir() {
        return Some(files);
    }
    let idx = tree.selected_file().or(tree.highlighted_file())?;
    let model = tree.model();
    let dir = model.ancestors_of_file(idx).pop()?;
    Some(model.dir(&dir)?.files.clone())
}

/// The toolbar's compact `N/M` (design §11.4), until the toolbar is narrow
/// enough to move it into the display options menu. Not a control: it
/// moves the window like the row. Its neighbours' `gap::CONTROLS` separate
/// it; tabular figures keep its width as the counts change.
pub fn progress_item(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    if tab.extension::<TabViewed>().is_none() || toolbar::narrow(tab) >= Narrow::ProgressInMenu {
        return None;
    }
    Some(
        div()
            .id("viewed-progress")
            .debug_selector(|| "viewed-progress".into())
            .flex_none()
            .text_style(text::UI)
            .font_features(crate::chrome::tabular_figures())
            .text_color(cx.theme().muted_foreground)
            .child(toolbar::text("viewed-progress-label", progress_label(tab)))
            .tooltip(toolbar::tooltip(progress_tooltip(tab)))
            .into_any_element(),
    )
}

/// The progress as the display options menu's first row, once it left the
/// toolbar.
pub fn progress_entry(tab: &ReviewTab) -> Option<MenuEntry> {
    tab.extension::<TabViewed>()
        .map(|_| MenuEntry::note(progress_tooltip(tab)))
}
