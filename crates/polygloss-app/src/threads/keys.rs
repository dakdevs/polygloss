//! The threads panel from the keyboard (T5.6, OQ-23): the panel takes the
//! keyboard (key context `ThreadsPanel`: `⇥` to it, or a click), `j`/`k`
//! (`↓`/`↑`) select a row, `⏎` goes to the thread (in the diff, or opens
//! it in the panel), `r` replies, `x` resolves or unresolves, `e` edits
//! your latest comment; "Delete my comment" is in the palette.
//!
//! The thread these act on ([`current_thread`]) is the panel's selected
//! row while the panel has the keyboard; from the diff it is the thread the
//! last `.`/`,` went to (while the cursor is still there), else the thread
//! on the cursor's line.

use gpui_kit::{App, Context, Entity, Window};
use polygloss_core::review::{ThreadStatus, ThreadView};

use super::placement::ThreadPlace;
use super::{ReviewThreads, activate_thread, go_to, panel, threads};
use crate::composer;
use crate::keymap::actions::threads as actions;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;

/// Registers the panel's actions on review tabs.
pub fn init(cx: &mut App) {
    handlers::on_action(cx, |tab: &mut ReviewTab, _: &actions::SelectNext, _, cx| {
        step(tab, 1, cx);
    });
    handlers::on_action(cx, |tab: &mut ReviewTab, _: &actions::SelectPrev, _, cx| {
        step(tab, -1, cx);
    });
    handlers::on_action(cx, |tab: &mut ReviewTab, _: &actions::Open, window, cx| {
        if let Some(id) = current_thread(tab, window, cx) {
            activate_thread(tab, &id, window, cx);
        }
    });
    handlers::on_action(cx, |tab: &mut ReviewTab, _: &actions::Reply, window, cx| {
        if let Some(id) = current_thread(tab, window, cx) {
            reply(tab, &id, window, cx);
        }
    });
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::ToggleResolved, window, cx| {
            let Some(id) = current_thread(tab, window, cx) else {
                return;
            };
            let Some(thread) = thread(tab, &id, cx) else {
                return;
            };
            // A draft thread is not visible to anyone yet: nothing to resolve.
            if !thread.draft {
                let resolved = thread.status == ThreadStatus::Resolved;
                composer::set_resolved(tab, &id, !resolved, cx);
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::EditComment, window, cx| {
            let Some(id) = current_thread(tab, window, cx) else {
                return;
            };
            if let Some(comment) = own_latest(tab, &id, cx) {
                reveal(tab, &id, cx);
                composer::open_edit(tab, &comment, window, cx);
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::DeleteComment, window, cx| {
            let Some(id) = current_thread(tab, window, cx) else {
                return;
            };
            if let Some(comment) = own_latest(tab, &id, cx) {
                composer::delete_comment(tab, &comment, window, cx);
            }
        },
    );
}

/// The thread the panel's actions act on (see the module docs).
pub fn current_thread(tab: &ReviewTab, window: &Window, cx: &App) -> Option<String> {
    let model = threads(tab)?.read(cx);
    if model.panel_focus.contains_focused(window, cx) {
        return model.selected.clone();
    }
    if tab.viewport_focus().contains_focused(window, cx) {
        let cursor = tab.viewport.read(cx).cursor();
        if let Some(mark) = &model.nav
            && mark.cursor == cursor
            && cursor.is_some()
        {
            return Some(mark.thread_id.clone());
        }
        if let Some(c) = cursor {
            let on_line = |t: &&ThreadView| {
                matches!(model.place(&t.id), Some(ThreadPlace::Line { file_idx, side, line, .. })
                    if *file_idx == c.file_idx && *side == c.side && *line == c.line)
            };
            let mut here: Vec<&ThreadView> = model.threads().filter(on_line).collect();
            here.sort_by_key(|t| t.status != ThreadStatus::Open);
            if let Some(t) = here.first() {
                return Some(t.id.clone());
            }
        }
    }
    model.selected.clone()
}

/// Gives the panel the keyboard, its first row selected when none is.
pub fn focus_panel(model: &Entity<ReviewThreads>, window: &mut Window, cx: &mut App) {
    let focus = model.read(cx).panel_focus.clone();
    window.focus(&focus, cx);
    let has_selection = {
        let m = model.read(cx);
        m.selected
            .as_ref()
            .is_some_and(|id| panel::rows(m, cx).iter().any(|(r, _)| r == id))
    };
    if !has_selection {
        model.update(cx, |m, cx| m.select_row(0, cx));
    }
}

/// `j` / `k`: the selection `delta` rows down or up (it stops at the ends).
fn step(tab: &mut ReviewTab, delta: isize, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads(tab).cloned() else {
        return;
    };
    let (rows, selected) = {
        let m = model.read(cx);
        (panel::rows(m, cx), m.selected.clone())
    };
    if rows.is_empty() {
        return;
    }
    let at = selected.and_then(|id| rows.iter().position(|(r, _)| *r == id));
    let ix = match at {
        Some(i) => i.saturating_add_signed(delta).min(rows.len() - 1),
        None if delta < 0 => rows.len() - 1,
        None => 0,
    };
    model.update(cx, |m, cx| m.select_row(ix, cx));
}

fn thread(tab: &ReviewTab, id: &str, cx: &App) -> Option<ThreadView> {
    threads(tab)?.read(cx).thread(id).cloned()
}

/// Your latest comment in thread `id` that is still there.
fn own_latest(tab: &ReviewTab, id: &str, cx: &App) -> Option<String> {
    thread(tab, id, cx)?
        .comments
        .iter()
        .rev()
        .find(|c| composer::own_comment(c))
        .map(|c| c.id.clone())
}

/// Makes thread `id`'s card show: a collapsed one (resolved, a note, a
/// panel-only thread) opens, and one in the diff scrolls into view.
fn reveal(tab: &mut ReviewTab, id: &str, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads(tab).cloned() else {
        return;
    };
    let (place, collapsed) = {
        let m = model.read(cx);
        let place = m.place(id).copied().unwrap_or(ThreadPlace::Panel);
        let panel_only = place == ThreadPlace::Panel || m.thread(id).is_some_and(|t| !m.shows(t));
        let collapsed = if panel_only {
            !m.is_expanded(id)
        } else {
            m.thread(id).is_some_and(|t| m.collapsed(t))
        };
        (place, collapsed)
    };
    if collapsed {
        model.update(cx, |m, cx| m.toggle_expanded(id, cx));
    }
    if place != ThreadPlace::Panel {
        go_to(tab, id, place, cx);
    }
}

/// `r`: a reply composer in thread `id`'s card, or, for another review's
/// thread, that review's tab (OQ-P16).
fn reply(tab: &mut ReviewTab, id: &str, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(thread) = thread(tab, id, cx) else {
        return;
    };
    if thread.review_id.as_deref() != Some(tab.review_id.as_str()) {
        let review_id = thread.review_id.clone().unwrap_or_default();
        let open =
            composer::composers(tab).and_then(|c| match c.read(cx).other_review(&review_id) {
                Some(composer::OtherReview::Known { open, .. }) => open.clone(),
                _ => None,
            });
        composer::open_other_review(&review_id, open, window, cx);
        return;
    }
    reveal(tab, id, cx);
    composer::open_reply(tab, id, window, cx);
}
