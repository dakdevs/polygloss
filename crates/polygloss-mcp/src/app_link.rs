//! What the write tools ask of the Polygloss app (design §13.3, §13.4, §15.1
//! "App dependency"): the `store_changed` nudge after every write, showing a
//! review (`open_diff` with `show`), scrolling it (`focus`) and the hidden
//! launch that lets `request_rereview` notify.
//!
//! Rules:
//!
//! - Only `open_diff` (with `show`), `focus` and `request_rereview` may launch
//!   the app; every other write only nudges an app that already runs.
//! - Agents never bring the app forward: `open` goes with `activate: false` and
//!   launches are background launches (`open -g`).
//! - The app being unavailable is an outcome, not an error: `open_diff` reports
//!   `app: "unavailable"`, `focus` `status: "unavailable"`.

use std::time::Duration;

use polygloss_core::ipc::client::IpcClient;
use polygloss_core::ipc::protocol::{IpcError, Op, codes};
use polygloss_core::store::events::latest_seq;
use polygloss_core::urls::{PolyglossUrl, format_url};
use polygloss_platform::launch::{LaunchOutcome, app_is_running, ensure_app};
use serde::Serialize;

use crate::context::{ApiContext, nudge_app};
use crate::errors::{ApiError, ApiErrorCode};

/// How long the app may take to answer `open` or `focus` (it may be resolving a
/// review first).
pub const APP_OP_TIMEOUT: Duration = Duration::from_secs(30);

/// What happened with the app when a tool asked it to show something
/// (`open_diff.app`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppShown {
    /// The running app opened (or focused) the tab.
    Opened,
    /// The app was launched in the background and opened the tab.
    Launched,
    /// Not asked (`show: false`).
    Skipped,
    /// The app could not be launched or did not answer. Not an error: the
    /// review is recorded and the human can open it later.
    Unavailable,
}

/// After a write: nudge a running app with the latest event seq (never
/// launches it).
pub fn after_write(ctx: &ApiContext) {
    match ctx.core.store.read(latest_seq) {
        Ok(seq) => nudge_app(ctx, seq),
        Err(e) => tracing::debug!("store_changed nudge: reading the latest seq: {e}"),
    }
}

/// Makes sure the app runs (launching it in the background when it does not)
/// and sends it `op`. `Ok(true)` when this call launched it.
fn run_op(ctx: &ApiContext, op: Op) -> Result<bool, AppCallError> {
    let launched = match ensure_app(&ctx.core.paths, None, false, ctx.launcher.as_ref()) {
        LaunchOutcome::AlreadyRunning => false,
        LaunchOutcome::Launched => true,
        LaunchOutcome::Unavailable(message) => return Err(AppCallError::Unavailable(message)),
    };
    let mut client = IpcClient::connect(&ctx.core.paths)
        .map_err(|e| AppCallError::Unavailable(e.message))?
        .ok_or_else(|| AppCallError::Unavailable("Polygloss stopped listening".into()))?;
    client
        .call(op, APP_OP_TIMEOUT)
        .map_err(AppCallError::from)?;
    Ok(launched)
}

/// Why an app op did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppCallError {
    /// No app: it could not be launched, or the connection failed.
    Unavailable(String),
    /// The app answered with an error of its own.
    App(IpcError),
}

impl From<IpcError> for AppCallError {
    fn from(e: IpcError) -> AppCallError {
        let transport = [
            codes::UNAVAILABLE,
            codes::TIMEOUT,
            codes::DISCONNECTED,
            codes::IO,
        ];
        if transport.contains(&e.code.as_str()) {
            AppCallError::Unavailable(e.message)
        } else {
            AppCallError::App(e)
        }
    }
}

/// Asks the app to open the review's tab in the background, launching it when
/// it is not running.
pub fn show_review(ctx: &ApiContext, review_id: &str) -> AppShown {
    let op = Op::Open {
        review_id: Some(review_id.to_owned()),
        diff_id: None,
        activate: false,
    };
    match run_op(ctx, op) {
        Ok(false) => AppShown::Opened,
        Ok(true) => AppShown::Launched,
        Err(e) => {
            tracing::debug!("showing review {review_id}: {e:?}");
            AppShown::Unavailable
        }
    }
}

/// `focus`'s outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusStatus {
    /// The running app scrolled to the location.
    Focused,
    /// The app was launched in the background, then scrolled.
    Launched,
    /// No app could be reached.
    Unavailable,
}

/// Sends `focus` (an [`Op::Focus`]), launching the app in the background when
/// needed. The app's own errors (an unknown path, a bad line) are returned with
/// their §15.1 code (`bad_request` is `conflict`).
pub fn focus(ctx: &ApiContext, op: Op) -> Result<FocusStatus, ApiError> {
    match run_op(ctx, op) {
        Ok(false) => Ok(FocusStatus::Focused),
        Ok(true) => Ok(FocusStatus::Launched),
        Err(AppCallError::Unavailable(message)) => {
            tracing::debug!("focus: the app is unavailable: {message}");
            Ok(FocusStatus::Unavailable)
        }
        Err(AppCallError::App(e)) => {
            let code = if e.code == codes::BAD_REQUEST {
                ApiErrorCode::Conflict
            } else {
                ApiErrorCode::parse(&e.code).unwrap_or(ApiErrorCode::Internal)
            };
            Err(ApiError::new(code, e.message))
        }
    }
}

/// `request_rereview`'s notification: a running app learns about the request
/// from its store feed (the nudge after the write); one that is not running is
/// launched in the background with the review's URL, whose handler posts the
/// notification (the feed of a fresh app starts after the request). Muted
/// reviews never launch the app. Returns whether it launched the app. Never
/// waits for the app and never fails.
pub fn launch_to_notify(ctx: &ApiContext, review_id: &str) -> bool {
    if app_is_running(&ctx.core.paths) {
        return false;
    }
    match ctx.core.review_summary(review_id) {
        Ok(Some(s)) if s.muted => return false,
        Ok(_) => {}
        Err(e) => tracing::debug!("reading review {review_id} before a launch: {e}"),
    }
    let url = format_url(&PolyglossUrl::Review(review_id.to_owned()));
    match ctx.launcher.launch(Some(&url), false) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("launching Polygloss to notify: {e}");
            false
        }
    }
}
