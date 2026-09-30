//! The app socket server wiring (design §13.3, §13.4; T4.1).
//!
//! `polygloss-core`'s [`serve`] runs the accept loop and one thread per
//! connection; its handler is an [`IpcBridge`], which forwards each op over
//! a `futures::channel::mpsc` to a GPUI foreground task and waits (up to
//! [`REPLY_TIMEOUT`]) for the answer. On the main thread [`handle`] carries
//! the op out:
//!
//! - `open` → the review's tab (`review_tab::open_review`, or the tab that
//!   is already open), brought forward when `activate`;
//! - `focus` → the same, then the cursor on the line or thread (never an
//!   external editor);
//! - `store_changed` → [`crate::feed::nudge`];
//! - `debug_state` (only with `POLYGLOSS_TEST=1`, which the core server
//!   enforces) → [`debug_state::snapshot`].
//!
//! [`startup::run`](crate::startup::run) calls [`serve_app`] once the main
//! window is open. Tests drive [`handle`] directly, or a bridge from
//! [`start_bridge`], which polls its channel every
//! [`WATCHER_POLL`](crate::app_state::WATCHER_POLL) when foreign threads may
//! not wake the app (GPUI's test scheduler).
//!
//! [`single_instance`] keeps unbundled dev builds to one app per data dir:
//! an `app.lock` flock plus socket liveness; a second instance forwards its
//! argv as `open` and exits 0.

pub mod debug_state;
pub mod ops;
pub mod single_instance;

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AsyncApp, Global};
use polygloss_core::ipc::{IpcError, Op, ServerHandle, codes, serve};
use serde_json::Value;

pub use ops::handle;

use crate::app_state::{AppState, WATCHER_POLL, watchers_wake_the_app};

/// How long a connection thread waits for the main thread to carry an op
/// out (opening a large review included). The op still completes after a
/// timeout; only its answer is lost.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Registers the socket feature (the server itself starts in
/// [`serve_app`]).
pub fn init(_cx: &mut App) {}

/// One op waiting for the main thread, and where its answer goes.
struct Pending {
    op: Op,
    reply: std::sync::mpsc::Sender<Result<Value, IpcError>>,
}

/// Hands ops from any thread to the main thread ([`start_bridge`]).
#[derive(Clone)]
pub struct IpcBridge {
    tx: mpsc::UnboundedSender<Pending>,
}

impl IpcBridge {
    /// Runs `op` on the main thread ([`handle`]) and waits up to `timeout`
    /// for its answer. Blocks: call it off the main thread.
    pub fn call(&self, op: Op, timeout: Duration) -> Result<Value, IpcError> {
        let (reply, answer) = std::sync::mpsc::channel();
        self.tx
            .unbounded_send(Pending { op, reply })
            .map_err(|_| IpcError::new(codes::UNAVAILABLE, "the app is quitting"))?;
        match answer.recv_timeout(timeout) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(IpcError::new(
                codes::TIMEOUT,
                format!("the app did not finish in {} s", timeout.as_secs()),
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(IpcError::new(
                codes::UNAVAILABLE,
                "the app dropped the request (quitting?)",
            )),
        }
    }
}

/// Starts the foreground task that carries out the ops sent through the
/// returned bridge, until every bridge is dropped.
pub fn start_bridge(cx: &mut App) -> IpcBridge {
    let (tx, mut rx) = mpsc::unbounded::<Pending>();
    if watchers_wake_the_app(cx) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(pending) = rx.next().await {
                cx.update(|cx| dispatch(pending, cx));
            }
        })
        .detach();
    } else {
        // GPUI's test scheduler panics when a foreign thread wakes a task:
        // poll instead (never registers a waker).
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(WATCHER_POLL).await;
                loop {
                    match rx.try_recv() {
                        Ok(pending) => cx.update(|cx| dispatch(pending, cx)),
                        Err(mpsc::TryRecvError::Closed) => return,
                        Err(mpsc::TryRecvError::Empty) => break,
                    }
                }
            }
        })
        .detach();
    }
    IpcBridge { tx }
}

/// Starts `pending`'s op and answers when it is done.
fn dispatch(pending: Pending, cx: &mut App) {
    let Pending { op, reply } = pending;
    let task = handle(op, cx);
    cx.spawn(async move |_: &mut AsyncApp| {
        // The connection may have given up waiting.
        let _ = reply.send(task.await);
    })
    .detach();
}

/// The running socket server (a GPUI global); dropped at quit, which
/// removes the socket file.
pub struct IpcServer {
    handle: Option<ServerHandle>,
}

impl Global for IpcServer {}

impl IpcServer {
    /// Where the server listens.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.handle.as_ref().map(ServerHandle::path)
    }
}

/// Starts the app's socket server on [`AppState`]'s data dir. A failure
/// (another app serves the socket, the directory is unusable) is logged and
/// the app runs without a socket.
pub fn serve_app(cx: &mut App) {
    let Some(state) = cx.try_global::<AppState>() else {
        return;
    };
    let paths = state.paths.clone();
    let bridge = start_bridge(cx);
    match serve(&paths, move |op| bridge.call(op, REPLY_TIMEOUT)) {
        Ok(handle) => {
            tracing::info!("listening on {}", handle.path().display());
            cx.set_global(IpcServer {
                handle: Some(handle),
            });
            cx.on_app_quit(|cx| {
                if cx.has_global::<IpcServer>() {
                    // Stops the accept loop and removes the socket file.
                    cx.global_mut::<IpcServer>().handle.take();
                }
                async {}
            })
            .detach();
        }
        Err(e) => tracing::warn!("the app socket is off: {e}"),
    }
}
