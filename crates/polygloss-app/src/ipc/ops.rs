//! Carrying socket ops out on the main thread ([`handle`]).
//!
//! A target names a review: `review_id`; else `diff_id` (a tab showing
//! that diff, else the most recently active review with an iteration of it,
//! `Core::latest_review_for_diff`); else `thread_id` (a tab whose threads
//! include it, else the thread's review). The review's tab is focused when
//! it is open, else opened like Home opens it (`home::row::open_request`,
//! then `review_tab::open_review`), in the main window (reopened when it
//! was closed). The tab keeps showing what it shows; positions of `focus`
//! apply to that diff (design §15.2: the review's latest iteration).
//!
//! Lines are 1-based, as on the wire.

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, Task};
use polygloss_core::ipc::{IpcError, Op, codes};
use polygloss_core::review::{Core, CoreError, ReviewSummary, Viewer};
use polygloss_diff::Side;
use serde_json::{Value, json};

use crate::app_state::AppState;
use crate::review_tab::{ReviewTab, open_review};
use crate::threads::placement::ThreadPlace;
use crate::threads::{self, ReviewThreads};
use crate::window::main_window;

/// How long `focus` waits for a new tab's threads to load before looking
/// for a thread in them.
const THREADS_LOAD_WAIT: Duration = Duration::from_secs(5);

/// Carries `op` out; the task resolves to the op's result or error.
pub fn handle(op: Op, cx: &mut App) -> Task<Result<Value, IpcError>> {
    match op {
        Op::Hello { .. } => Task::ready(Ok(polygloss_core::ipc::server::hello())),
        Op::StoreChanged { .. } => {
            crate::feed::nudge(cx);
            Task::ready(Ok(json!({ "status": "nudged" })))
        }
        Op::DebugState => Task::ready(Ok(super::debug_state::snapshot(cx))),
        Op::Open {
            review_id,
            diff_id,
            activate,
        } => {
            let target = Target {
                review_id,
                diff_id,
                thread_id: None,
            };
            cx.spawn(async move |cx: &mut AsyncApp| open(target, activate, cx).await)
        }
        Op::Focus {
            review_id,
            diff_id,
            path,
            side,
            line,
            thread_id,
        } => {
            let target = Target {
                review_id,
                diff_id,
                thread_id: thread_id.clone(),
            };
            let place = Place {
                path,
                side,
                line,
                thread_id,
            };
            cx.spawn(async move |cx: &mut AsyncApp| focus(target, place, cx).await)
        }
    }
}

/// Which review an op is about.
#[derive(Debug, Clone)]
struct Target {
    review_id: Option<String>,
    diff_id: Option<String>,
    thread_id: Option<String>,
}

impl Target {
    fn is_empty(&self) -> bool {
        self.review_id.is_none() && self.diff_id.is_none() && self.thread_id.is_none()
    }
}

/// Where `focus` goes in the tab.
#[derive(Debug, Clone)]
struct Place {
    path: Option<String>,
    side: Option<Side>,
    line: Option<u32>,
    thread_id: Option<String>,
}

async fn open(target: Target, activate: bool, cx: &mut AsyncApp) -> Result<Value, IpcError> {
    cx.update(|cx| bring_forward(activate, cx));
    if target.is_empty() {
        return Ok(json!({ "status": "activated" }));
    }
    let (tab, status) = show_tab(target, cx).await?;
    Ok(cx.update(|cx| tab_result(status, &tab, cx)))
}

async fn focus(target: Target, place: Place, cx: &mut AsyncApp) -> Result<Value, IpcError> {
    if target.is_empty() {
        return Err(bad_request("focus needs review_id, diff_id or thread_id"));
    }
    if place.line == Some(0) {
        return Err(bad_request("lines are 1-based"));
    }
    if place.line.is_some() && place.path.is_none() && place.thread_id.is_none() {
        return Err(bad_request("focus with a line needs its path"));
    }
    cx.update(|cx| bring_forward(false, cx));
    let (tab, _) = show_tab(target, cx).await?;
    if let Some(thread_id) = &place.thread_id {
        focus_thread(&tab, thread_id, cx).await?;
    } else if let Some(path) = &place.path {
        cx.update(|cx| tab.update(cx, |t, cx| focus_path(t, path, place.side, place.line, cx)))?;
    }
    Ok(cx.update(|cx| tab_result("focused", &tab, cx)))
}

fn tab_result(status: &str, tab: &Entity<ReviewTab>, cx: &App) -> Value {
    let t = tab.read(cx);
    json!({
        "status": status,
        "review_id": t.review_id,
        "diff_id": t.opened.diff_id.as_str(),
    })
}

/// Makes sure the main window is open (the tab needs it) and, when
/// `activate`, brings the app and the window to the front.
fn bring_forward(activate: bool, cx: &mut App) {
    crate::window::reopen(cx);
    if activate {
        crate::window::activate_app(cx);
        if let Some((handle, _)) = main_window(cx) {
            handle
                .update(cx, |_, window, _| window.activate_window())
                .ok();
        }
    }
}

/// The target's tab, focused in the main window: `"focused"` when it was
/// open, `"opened"` when it was opened for this op.
async fn show_tab(
    target: Target,
    cx: &mut AsyncApp,
) -> Result<(Entity<ReviewTab>, &'static str), IpcError> {
    if let Some(tab) = cx.update(|cx| find_tab(&target, cx)) {
        cx.update(|cx| activate_tab(&tab, cx));
        return Ok((tab, "focused"));
    }
    let core = cx.update(|cx| AppState::global(cx).core.clone());
    let summary = cx
        .background_spawn(async move { lookup(&core, &target) })
        .await?;
    if let Some(tab) = cx.update(|cx| review_tab(&summary.review_id, cx)) {
        cx.update(|cx| activate_tab(&tab, cx));
        return Ok((tab, "focused"));
    }
    let req = crate::home::row::open_request(&summary).ok_or_else(|| {
        IpcError::new(
            codes::INTERNAL,
            format!(
                "cannot open review {}: unknown key {:?}",
                summary.review_id, summary.key
            ),
        )
    })?;
    let opening = cx
        .update(|cx| {
            let (handle, _) = main_window(cx)?;
            handle
                .update(cx, |_, window, cx| open_review(req, window, cx))
                .ok()
        })
        .ok_or_else(|| IpcError::new(codes::UNAVAILABLE, "the main window could not open"))?;
    let tab = opening.await.map_err(open_error)?;
    Ok((tab, "opened"))
}

/// An open tab for `target`, without reading the store.
fn find_tab(target: &Target, cx: &App) -> Option<Entity<ReviewTab>> {
    if let Some(review_id) = &target.review_id {
        return review_tab(review_id, cx);
    }
    let (_, main) = main_window(cx)?;
    let tabs = main.read(cx).tabs().items().to_vec();
    let mut reviews = tabs.iter().filter_map(|item| item.review());
    if let Some(diff_id) = &target.diff_id {
        return reviews
            .find(|t| t.read(cx).opened.diff_id.as_str() == diff_id)
            .cloned();
    }
    let thread_id = target.thread_id.as_deref()?;
    reviews
        .find(|t| {
            threads::threads(t.read(cx)).is_some_and(|m| m.read(cx).thread(thread_id).is_some())
        })
        .cloned()
}

/// The open tab of review `review_id`.
fn review_tab(review_id: &str, cx: &App) -> Option<Entity<ReviewTab>> {
    let (_, main) = main_window(cx)?;
    let main = main.read(cx);
    let ix = main.tabs().find_review(review_id, cx)?;
    main.tabs().get(ix)?.review().cloned()
}

/// Activates `tab` in the main window.
fn activate_tab(tab: &Entity<ReviewTab>, cx: &mut App) {
    let Some((handle, main)) = main_window(cx) else {
        return;
    };
    let Some(ix) = main
        .read(cx)
        .tabs()
        .items()
        .iter()
        .position(|item| item.review() == Some(tab))
    else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            main.update(cx, |m, cx| m.activate_tab(ix, window, cx))
        })
        .ok();
}

/// The review `target` names, from the store (off the main thread).
fn lookup(core: &Core, target: &Target) -> Result<ReviewSummary, IpcError> {
    let review_id = if let Some(id) = &target.review_id {
        id.clone()
    } else if let Some(diff_id) = &target.diff_id {
        core.latest_review_for_diff(diff_id)
            .map_err(core_error)?
            .ok_or_else(|| not_found(format!("no review has diff {diff_id}")))?
    } else if let Some(thread_id) = &target.thread_id {
        core.thread(thread_id, Viewer::Human)
            .map_err(core_error)?
            .review_id
            .ok_or_else(|| not_found(format!("thread {thread_id} has no review")))?
    } else {
        return Err(bad_request("name a review_id, diff_id or thread_id"));
    };
    core.review_summary(&review_id)
        .map_err(core_error)?
        .ok_or_else(|| not_found(format!("no review {review_id}")))
}

/// Puts the cursor on `line` (1-based) of `path` on `side` (default new), or
/// scrolls to the file when there is no line.
fn focus_path(
    tab: &mut ReviewTab,
    path: &str,
    side: Option<Side>,
    line: Option<u32>,
    cx: &mut Context<ReviewTab>,
) -> Result<(), IpcError> {
    let side = side.unwrap_or(Side::New);
    let files = tab.viewport.read(cx).document().files().clone();
    let file_idx = files
        .iter()
        .position(|f| {
            f.display_path() == path
                || (side == Side::Old && f.old_path.as_ref().is_some_and(|p| p.text == path))
        })
        .ok_or_else(|| not_found(format!("{path} is not in the diff this tab shows")))?;
    let file_idx = u32::try_from(file_idx).unwrap_or(u32::MAX);
    match line {
        Some(line) => {
            let line = line.saturating_sub(1);
            threads::go_to(
                tab,
                "",
                ThreadPlace::Line {
                    file_idx,
                    side,
                    start_line: line,
                    line,
                },
                cx,
            );
        }
        None => tab.viewport.update(cx, |v, cx| {
            v.set_collapsed(file_idx, false, cx);
            v.go_to_file(file_idx, cx);
        }),
    }
    Ok(())
}

/// Shows thread `id` in `tab`: the cursor on it in the diff, or open in the
/// threads panel when it is not placed in the diff.
async fn focus_thread(
    tab: &Entity<ReviewTab>,
    id: &str,
    cx: &mut AsyncApp,
) -> Result<(), IpcError> {
    let model = cx
        .update(|cx| threads::threads(tab.read(cx)).cloned())
        .ok_or_else(|| IpcError::new(codes::INTERNAL, "the tab has no threads"))?;
    wait_loaded(&model, cx).await;
    cx.update(|cx| {
        let (place, shows, collapsed, expanded) = {
            let m = model.read(cx);
            let thread = m.thread(id).ok_or_else(|| {
                not_found(format!("thread {id} is not in the diff this tab shows"))
            })?;
            (
                m.place(id).copied().unwrap_or(ThreadPlace::Panel),
                m.shows(thread),
                m.collapsed(thread),
                m.is_expanded(id),
            )
        };
        tab.update(cx, |t, cx| {
            if place == ThreadPlace::Panel || !shows {
                if !expanded {
                    model.update(cx, |m, cx| m.toggle_expanded(id, cx));
                }
                if !t.threads_panel_visible() {
                    t.toggle_threads_panel(cx);
                }
            } else {
                if collapsed {
                    model.update(cx, |m, cx| m.toggle_expanded(id, cx));
                }
                threads::go_to(t, id, place, cx);
            }
        });
        Ok(())
    })
}

/// Waits until `model` has loaded (at most [`THREADS_LOAD_WAIT`]).
async fn wait_loaded(model: &Entity<ReviewThreads>, cx: &mut AsyncApp) {
    let (tx, mut notified) = mpsc::unbounded::<()>();
    let _subscription = cx.update(|cx| {
        cx.observe(model, move |_, _| {
            let _ = tx.unbounded_send(());
        })
    });
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    while !cx.update(|cx| model.read(cx).is_loaded()) && waited < THREADS_LOAD_WAIT {
        let timer = cx.background_executor().timer(step);
        if let futures::future::Either::Right(_) =
            futures::future::select(notified.next(), timer).await
        {
            waited += step;
        }
    }
}

fn core_error(e: CoreError) -> IpcError {
    IpcError::new(e.code(), e.to_string())
}

/// A failed `open_review` (its message is the one the window shows).
fn open_error(err: anyhow::Error) -> IpcError {
    let code = err
        .chain()
        .find_map(|e| e.downcast_ref::<CoreError>())
        .map_or(codes::INTERNAL, CoreError::code);
    IpcError::new(code, format!("{err:#}"))
}

fn not_found(message: String) -> IpcError {
    IpcError::new(codes::NOT_FOUND, message)
}

fn bad_request(message: &str) -> IpcError {
    IpcError::new(codes::BAD_REQUEST, message)
}
