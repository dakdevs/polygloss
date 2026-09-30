//! `polygloss://` URLs (T4.2, design §13.5).
//!
//! macOS hands the app its URLs through `application:openURLs:` (GPUI's
//! `Application::on_open_urls`), which must be registered before `run()` and
//! has no `App` context: [`register`] queues them in a [`UrlInbox`] that
//! [`listen`] drains once the app runs. `polygloss://` launch arguments (the
//! unbundled app started by `$POLYGLOSS_APP_BIN` or `cargo run`) go through
//! the same [`open_url`].
//!
//! [`open_url`] parses the URL, resolves it in the store off the main thread
//! (`Core::resolve_url`: the review tab it opens, the iteration a diff URL
//! names and what to focus), then focuses that review's tab or opens it.
//! A diff URL shows its diff, not whatever the review shows now: a tab that
//! shows the diff already is used as it is (also an unpinned live state,
//! which the store has no iteration for); else the review's tab switches to
//! the iteration showing it, as the iteration picker does. Finally it
//! scrolls to the file, the line (with the line cursor on it) or the thread. It never activates the
//! app itself: a clicked link comes forward through LaunchServices, while a
//! background launch (`open -g`) stays in the background. Errors show in the
//! window like a failed open.

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Application, AsyncApp, Context, Entity, Subscription,
    Task, Window,
};
use polygloss_core::review::ReviewSummary;
use polygloss_core::urls::{PolyglossUrl, UrlDiff, UrlFocus, UrlTarget, parse_url};
use polygloss_viewport::{CursorPos, ScrollTarget};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::app_state::AppState;
use crate::home::row;
use crate::iterations::{self, Choice};
use crate::review_tab::{ReviewTab, open_review};
use crate::threads::{self, placement::ThreadPlace};

/// The module map's per-feature hook (`features::init` calls every module's,
/// plan M3 "App module map"). URLs need nothing per app: the platform
/// callback must be registered before `run()` ([`register`]).
pub fn init(_cx: &mut App) {}

/// Sends URLs to a [`UrlInbox`].
pub type UrlSender = UnboundedSender<Vec<String>>;

/// URLs received before [`listen`] runs wait here.
pub struct UrlInbox {
    tx: UrlSender,
    rx: UnboundedReceiver<Vec<String>>,
}

impl UrlInbox {
    pub fn new() -> UrlInbox {
        let (tx, rx) = unbounded();
        UrlInbox { tx, rx }
    }

    /// A sender for this inbox (the platform callback, launch arguments).
    pub fn sender(&self) -> UrlSender {
        self.tx.clone()
    }
}

impl Default for UrlInbox {
    fn default() -> UrlInbox {
        UrlInbox::new()
    }
}

/// Registers `application:openURLs:` on `app`; call before `run()`.
pub fn register(app: &Application) -> UrlInbox {
    let inbox = UrlInbox::new();
    let tx = inbox.sender();
    app.on_open_urls(move |urls| {
        // Fails only once the app has quit.
        let _ = tx.unbounded_send(urls);
    });
    inbox
}

/// Opens every URL the inbox receives, in order, for as long as the app runs.
pub fn listen(inbox: UrlInbox, cx: &mut App) {
    let UrlInbox { tx, mut rx } = inbox;
    // The inbox's own sender would keep the stream open forever; the
    // platform's clone does that for as long as the app runs.
    drop(tx);
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Some(urls) = rx.next().await {
            for url in urls {
                // Errors are shown in the window by `open_url`.
                let _ = cx.update(|cx| open_url(&url, cx)).await;
            }
        }
    })
    .detach();
}

/// Opens `url`: focuses (or opens) the review tab it names, shows the diff a
/// diff URL names (module docs), then focuses its file, line or thread.
/// Errors are shown in the main window and returned.
pub fn open_url(url: &str, cx: &mut App) -> Task<anyhow::Result<()>> {
    tracing::info!("opening {url}");
    let parsed = parse_url(url);
    let core = cx.try_global::<AppState>().map(|s| s.core.clone());
    let url = url.to_owned();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = async {
            let parsed = parsed?;
            let core = core.ok_or_else(|| anyhow::anyhow!("the store is not open"))?;
            // A tab that shows the diff already: an unpinned live state has no
            // iteration for the store to find it by.
            let showing = match &parsed {
                PolyglossUrl::Diff { diff_id, .. } => cx.update(|cx| tab_showing(diff_id, cx)),
                _ => None,
            };
            let (handle, tab, diff, focus) = match showing {
                Some((handle, ix, tab)) => {
                    handle.update(cx, |_, window, cx| {
                        if let Some((_, main)) = crate::window::main_window(cx) {
                            main.update(cx, |main, cx| main.activate_tab(ix, window, cx));
                        }
                    })?;
                    (handle, tab, None, parsed.focus())
                }
                None => {
                    let (target, summary) = cx
                        .background_spawn(async move {
                            let target = core.resolve_url(&parsed)?;
                            let summary = core.review_summary(&target.review_id)?;
                            anyhow::Ok((target, summary))
                        })
                        .await?;
                    let (handle, tab) = open_review_tab(&target, summary, cx)?;
                    (handle, tab.await?, target.diff, target.focus)
                }
            };
            if let Some(diff) = diff {
                show_diff(handle, &tab, &diff, cx).await?;
            }
            if let Some(focus) = focus {
                handle.update(cx, |_, window, cx| apply_focus(&tab, focus, window, cx))??;
            }
            anyhow::Ok(())
        }
        .await;
        if let Err(err) = &result {
            let message = format!("Could not open {url}: {err:#}");
            tracing::warn!("{message}");
            cx.update(|cx| show_error(message, cx));
        }
        result
    })
}

/// The main window, the index and the tab of an open review tab that shows
/// diff `diff_id`.
fn tab_showing(diff_id: &str, cx: &mut App) -> Option<(AnyWindowHandle, usize, Entity<ReviewTab>)> {
    let (handle, main) = crate::window::main_window(cx)?;
    let tabs = main.read(cx).tabs();
    let (ix, tab) = tabs.items().iter().enumerate().find_map(|(ix, t)| {
        let tab = t.review()?;
        (tab.read(cx).opened.diff_id.as_str() == diff_id).then(|| (ix, tab.clone()))
    })?;
    Some((handle, ix, tab))
}

/// Focuses the tab of `target`'s review, or opens it (reopening the window
/// if it was closed); the tab once it is up.
fn open_review_tab(
    target: &UrlTarget,
    summary: Option<ReviewSummary>,
    cx: &mut AsyncApp,
) -> anyhow::Result<(AnyWindowHandle, Task<anyhow::Result<Entity<ReviewTab>>>)> {
    cx.update(|cx| {
        crate::window::reopen(cx);
        let (handle, main) = crate::window::main_window(cx)
            .ok_or_else(|| anyhow::anyhow!("the main window is closed"))?;
        let tab = handle.update(cx, |_, window, cx| {
            let open = main.read(cx).tabs().find_review(&target.review_id, cx);
            if let Some(ix) = open {
                main.update(cx, |main, cx| main.activate_tab(ix, window, cx));
                let tab = main
                    .read(cx)
                    .tabs()
                    .get(ix)
                    .and_then(|t| t.review())
                    .cloned();
                return Ok(Task::ready(
                    tab.ok_or_else(|| anyhow::anyhow!("tab {ix} is not a review")),
                ));
            }
            let summary =
                summary.ok_or_else(|| anyhow::anyhow!("review {} is gone", target.review_id))?;
            // Opens the review's current state (a compare review on refs that
            // moved records its next iteration, as any open does); `show_diff`
            // then switches to the iteration the URL names.
            let req = row::open_request(&summary)
                .ok_or_else(|| anyhow::anyhow!("cannot open review key {:?}", summary.key))?;
            anyhow::Ok(open_review(req, window, cx))
        })??;
        Ok((handle, tab))
    })
}

/// Makes `tab` show `diff`: nothing when it does already; its current state
/// when that is the diff; else iteration `diff.seq`, as the iteration picker
/// shows it. An error when the tab does not show the diff afterwards.
async fn show_diff(
    handle: AnyWindowHandle,
    tab: &Entity<ReviewTab>,
    diff: &UrlDiff,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let switching = handle.update(cx, |_, window, cx| {
        tab.update(cx, |t, cx| {
            if t.opened.diff_id == diff.diff_id {
                return None;
            }
            let choice = if iterations::current(t).diff_id == diff.diff_id {
                Choice::Current
            } else {
                Choice::Iteration(diff.seq)
            };
            Some(iterations::show_then(t, choice, window, cx))
        })
    })?;
    if let Some(switching) = switching {
        switching.await;
    }
    let shown = cx.update(|cx| tab.read(cx).opened.diff_id == diff.diff_id);
    anyhow::ensure!(
        shown,
        "could not show iteration {} of the review (diff {})",
        diff.seq,
        diff.diff_id
    );
    Ok(())
}

/// Reports a URL that could not be opened in the main window.
fn show_error(message: String, cx: &mut App) {
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| main.show_error(message, window, cx))
    });
}

/// Scrolls `tab` to `focus`; a thread once the tab's threads have loaded.
pub fn apply_focus(
    tab: &Entity<ReviewTab>,
    focus: UrlFocus,
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<()> {
    match focus {
        UrlFocus::Thread(id) => {
            tab.update(cx, |tab, cx| focus_thread(tab, id, window, cx));
            Ok(())
        }
        UrlFocus::File { path } => {
            let file_idx = file_index(tab, &path, cx)?;
            tab.update(cx, |tab, cx| {
                tab.viewport.update(cx, |v, cx| {
                    v.set_collapsed(file_idx, false, cx);
                    v.scroll_to(ScrollTarget::File(file_idx), cx);
                })
            });
            Ok(())
        }
        UrlFocus::Line { path, side, line } => {
            let file_idx = file_index(tab, &path, cx)?;
            // URLs count from 1 (design §8.1), the viewport from 0.
            let line = line.saturating_sub(1);
            tab.update(cx, |tab, cx| {
                tab.viewport.update(cx, |v, cx| {
                    v.set_collapsed(file_idx, false, cx);
                    v.reveal_line(file_idx, side, line, cx);
                    v.set_cursor(
                        Some(CursorPos {
                            file_idx,
                            side,
                            line,
                            range_start: None,
                        }),
                        cx,
                    );
                    if v.document().file_layout(file_idx).is_none() {
                        // Not laid out yet: lands once it is.
                        v.scroll_to(
                            ScrollTarget::Line {
                                file_idx,
                                side,
                                line,
                            },
                            cx,
                        );
                    }
                });
                window.focus(&tab.viewport_focus().clone(), cx);
            });
            Ok(())
        }
    }
}

/// The index of `path` (new or old path) in the tab's diff.
fn file_index(tab: &Entity<ReviewTab>, path: &str, cx: &App) -> anyhow::Result<u32> {
    let files = &tab.read(cx).opened.files;
    files
        .iter()
        .find(|f| {
            [&f.new_path, &f.old_path]
                .into_iter()
                .flatten()
                .any(|p| p.text == path)
        })
        .map(|f| f.idx)
        .ok_or_else(|| anyhow::anyhow!("{path} is not in this diff"))
}

/// A thread focus waiting for the tab's threads to load (dropping it cancels).
struct PendingThreadFocus(Option<Subscription>);

/// Jumps to thread `id` as the threads panel does, once the tab's threads have
/// loaded. A thread without a place in the diff opens in the panel.
fn focus_thread(tab: &mut ReviewTab, id: String, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads::threads(tab).cloned() else {
        return;
    };
    if model.read(cx).is_loaded() {
        go_to_thread(tab, &id, window, cx);
        return;
    }
    let sub = cx.observe_in(&model, window, move |tab, model, window, cx| {
        if !model.read(cx).is_loaded() {
            return;
        }
        if let Some(pending) = tab.extension_mut::<PendingThreadFocus>() {
            pending.0.take();
        }
        go_to_thread(tab, &id, window, cx);
    });
    // Replaces (cancels) an earlier pending focus.
    tab.insert_extension(PendingThreadFocus(Some(sub)));
}

fn go_to_thread(tab: &mut ReviewTab, id: &str, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let in_panel = threads::threads(tab)
        .and_then(|m| m.read(cx).place(id).copied())
        .is_none_or(|place| place == ThreadPlace::Panel);
    if in_panel && !tab.threads_panel_visible() {
        tab.toggle_threads_panel(cx);
    }
    threads::activate_thread(tab, id, window, cx);
}
