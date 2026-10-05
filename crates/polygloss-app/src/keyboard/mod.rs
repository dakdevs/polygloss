//! Keyboard-only use (T5.6, OQ-23): every action is reachable from the
//! keyboard, the focused pane shows a focus ring, `Tab` / `⇧Tab` cycle a
//! review tab's panes and `Esc` closes every popover and dialog.
//!
//! - Pane cycle (`tab::FocusNextPane` / `FocusPrevPane`, `⇥` / `⇧⇥` in a
//!   review tab): the file tree (or the find results in its place, while
//!   the sidebar shows Files) → the diff → the threads panel (while shown)
//!   → every open composer, in the order they opened, and around. The
//!   composers' text fields give `⇥` to the cycle instead of indenting
//!   (`⌘]` / `⌘[` still indent). Moving the keyboard into the tree shows
//!   the sidebar's Files first ([`crate::chrome::show_files`]).
//! - [`menu`]: popup menus opened from the keyboard (the tree's filters,
//!   the iteration picker) next to the button the mouse opens them with.
//! - [`focus_step`]: `⇥` out of a multi-line text field inside a dialog
//!   (the Submit review summary) to the dialog's next control.

pub mod menu;

use gpui_kit::{App, Context, Focusable as _, Window};

use crate::composer::{self, ComposerKey};
use crate::keymap::actions::tab as tab_actions;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;

/// The panes of a review tab `Tab` / `⇧Tab` visit, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pane {
    /// The file tree (or the find results in its place), filter box
    /// included.
    Tree,
    Viewport,
    Threads,
    /// An open composer.
    Composer(ComposerKey),
}

/// Registers the pane cycle.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::FocusNextPane, window, cx| {
            cycle(tab, true, window, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::FocusPrevPane, window, cx| {
            cycle(tab, false, window, cx);
        },
    );
}

/// The pane of `tab` that holds the keyboard, if any. Composers come first:
/// they sit inside the viewport and the threads panel.
pub fn focused_pane(tab: &ReviewTab, window: &Window, cx: &App) -> Option<Pane> {
    if let Some(composers) = composer::composers(tab) {
        for key in composers.read(cx).keys() {
            if composer::composer(tab, &key, cx)
                .is_some_and(|c| c.read(cx).contains_focus(window, cx))
            {
                return Some(Pane::Composer(key));
            }
        }
    }
    if crate::threads::threads(tab)
        .is_some_and(|m| m.read(cx).panel_focus().contains_focused(window, cx))
    {
        return Some(Pane::Threads);
    }
    if tab.viewport_focus().contains_focused(window, cx) {
        return Some(Pane::Viewport);
    }
    let find = crate::find::find_bar(tab).is_some_and(|b| b.read(cx).contains_focus(window, cx));
    let tree = crate::tree::file_tree(tab).is_some_and(|t| t.read(cx).contains_focus(window, cx));
    (find || tree).then_some(Pane::Tree)
}

/// The panes `Tab` visits in `tab` now, in order: the tree only while the
/// sidebar shows it.
pub fn stops(tab: &ReviewTab, cx: &App) -> Vec<Pane> {
    let chrome = crate::chrome::chrome(cx).read(cx);
    let tree_shown = chrome.sidebar_visible() && chrome.segment() == crate::chrome::Segment::Files;
    let mut out = Vec::new();
    if tree_shown {
        out.push(Pane::Tree);
    }
    out.push(Pane::Viewport);
    if tab.threads_panel_visible() && crate::threads::threads(tab).is_some() {
        out.push(Pane::Threads);
    }
    if let Some(composers) = composer::composers(tab) {
        out.extend(composers.read(cx).keys().into_iter().map(Pane::Composer));
    }
    out
}

/// Moves the keyboard to the next (`forward`) or previous pane of `tab`.
/// From outside every pane (a toolbar button) `Tab` starts at the tree and
/// `⇧Tab` at the last pane.
pub fn cycle(tab: &mut ReviewTab, forward: bool, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let stops = stops(tab, cx);
    let current = focused_pane(tab, window, cx).and_then(|p| stops.iter().position(|s| *s == p));
    let len = stops.len();
    let next = match (current, forward) {
        (Some(i), true) => (i + 1) % len,
        (Some(i), false) => (i + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    };
    focus_pane(tab, &stops[next], window, cx);
}

/// Gives `pane` of `tab` the keyboard.
pub fn focus_pane(
    tab: &mut ReviewTab,
    pane: &Pane,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    match pane {
        // The find bar while it is open in the tree's place, else the tree
        // list; the sidebar shows them first.
        Pane::Tree => {
            crate::chrome::show_files(window, cx);
            match crate::find::find_bar(tab).filter(|b| b.read(cx).is_open()) {
                Some(bar) => {
                    let focus = bar.read(cx).input().focus_handle(cx);
                    window.focus(&focus, cx);
                }
                None => {
                    if let Some(tree) = crate::tree::file_tree(tab).cloned() {
                        tree.update(cx, |t, cx| t.focus(window, cx));
                    }
                }
            }
        }
        Pane::Viewport => window.focus(&tab.viewport_focus().clone(), cx),
        Pane::Threads => {
            if let Some(model) = crate::threads::threads(tab).cloned() {
                crate::threads::focus_panel(&model, window, cx);
            }
        }
        Pane::Composer(key) => {
            if let Some(view) = composer::composer(tab, key, cx) {
                let focus = view.read(cx).focus_target(cx);
                window.focus(&focus, cx);
            }
        }
    }
    cx.notify();
}

/// `⇥` (`forward`) or `⇧⇥` from a control inside a dialog: the dialog's
/// next or previous focusable control, around within the dialog (its focus
/// trap), as gpui-kit's root does for controls that do not take `⇥`.
pub fn focus_step(forward: bool, window: &mut Window, cx: &mut App) {
    let step = |window: &mut Window, cx: &mut App| {
        if forward {
            window.focus_next(cx)
        } else {
            window.focus_prev(cx)
        }
    };
    let Some(trap) = gpui_kit::base::active_focus_trap(window, cx) else {
        step(window, cx);
        return;
    };
    let before = window.focused(cx);
    step(window, cx);
    let mut attempts = 0;
    while !trap.contains_focused(window, cx) && attempts < 100 {
        step(window, cx);
        attempts += 1;
        if window.focused(cx) == before {
            break;
        }
    }
}
