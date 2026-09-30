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
//! (`Core::resolve_url`: the review tab it opens and what to focus), then
//! focuses that review's tab or opens it, and finally scrolls to the file,
//! the line (with the line cursor on it) or the thread. It never activates the
//! app itself: a clicked link comes forward through LaunchServices, while a
//! background launch (`open -g`) stays in the background. Errors show in the
//! window like a failed open.

use gpui_kit::{
    App, AppContext as _, Application, AsyncApp, Context, Entity, Subscription, Task, Window,
};
use polygloss_core::urls::{UrlFocus, parse_url};
use polygloss_viewport::{CursorPos, ScrollTarget};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::app_state::AppState;
use crate::home::row;
use crate::review_tab::{ReviewTab, open_review};
use crate::threads::{self, placement::ThreadPlace};

/// Registers URL handling (nothing to do per app; see [`register`]).
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

/// Opens `url`: focuses (or opens) the review tab it names, then its file,
/// line or thread. Errors are shown in the main window and returned.
pub fn open_url(url: &str, cx: &mut App) -> Task<anyhow::Result<()>> {
    tracing::info!("opening {url}");
    let parsed = parse_url(url);
    let core = cx.try_global::<AppState>().map(|s| s.core.clone());
    let url = url.to_owned();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = async {
            let parsed = parsed?;
            let core = core.ok_or_else(|| anyhow::anyhow!("the store is not open"))?;
            let (target, summary) = cx
                .background_spawn(async move {
                    let target = core.resolve_url(&parsed)?;
                    let summary = core.review_summary(&target.review_id)?;
                    anyhow::Ok((target, summary))
                })
                .await?;
            let (handle, tab) = cx.update(|cx| -> anyhow::Result<_> {
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
                    let summary = summary
                        .ok_or_else(|| anyhow::anyhow!("review {} is gone", target.review_id))?;
                    let req = row::open_request(&summary).ok_or_else(|| {
                        anyhow::anyhow!("cannot open review key {:?}", summary.key)
                    })?;
                    anyhow::Ok(open_review(req, window, cx))
                })??;
                Ok((handle, tab))
            })?;
            let tab = tab.await?;
            if let Some(focus) = target.focus {
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
