//! Opt-in `claude/channel` push (T4.13, design §16.4, OQ-33).
//!
//! Only with [`ServeOptions::channel`] (`polygloss mcp --channel` or
//! `POLYGLOSS_MCP_CHANNEL=1`); the plugin never enables it. Then:
//!
//! - [`declare`] adds `experimental["claude/channel"] = {}` to the server
//!   capabilities, which is what makes Claude Code register a channel listener
//!   (research preview: only on the stdio `initialize` handshake, and only when
//!   Claude Code runs with `--dangerously-load-development-channels server:<name>`).
//! - [`start`] (after `notifications/initialized`) watches the event feed from
//!   the latest `seq` at that moment and sends one
//!   `notifications/claude/channel` per new `review.submitted` on a review
//!   assigned to this server's session (its canonical session, §16.4), as
//!   `{content, meta: {review_id, submission_id, verdict}}` ([`notification`]).
//!   Which submissions count and the text are `polygloss wait`'s
//!   ([`crate::wake::wakes_for`]): submissions only (OQ-11), never agent
//!   replies; one per review per poll (its latest submission).
//!
//! Rules:
//!
//! - Claude Code never acknowledges a channel notification and drops it
//!   silently when the session did not load this server as a channel, so the
//!   push never sets `sessions.last_woken_seq`: the Stop-hook waiter still
//!   reports the submission. With both on, the agent may be told twice; a lost
//!   wake-up would be worse.
//! - Submissions from before the server was initialized are not pushed (the
//!   hook, `wait_for_review` or `list_reviews` report them).
//! - The watch polls every [`POLL`] (a `PRAGMA data_version` read while
//!   nothing changes) and stops when the transport closes. A store error is
//!   logged (warn once per failing streak) and the next poll retries.

use std::time::Duration;

use polygloss_core::review::{Core, CoreError};
use polygloss_core::store::events::{EventFeed, EventFilter, EventKind};
use rmcp::model::{CustomNotification, JsonObject, ServerCapabilities, ServerNotification};
use rmcp::{Peer, RoleServer};
use serde_json::json;

use crate::server::ServeOptions;
use crate::wake::{self, Wake};

/// The experimental capability key Claude Code looks for.
pub const CHANNEL_CAPABILITY: &str = "claude/channel";

/// The notification method Claude Code listens to on a channel server.
pub const CHANNEL_METHOD: &str = "notifications/claude/channel";

/// How often the event feed is polled.
pub const POLL: Duration = Duration::from_millis(250);

/// Adds `experimental["claude/channel"] = {}` to `caps` when `opts.channel` is set.
pub fn declare(opts: &ServeOptions, caps: &mut ServerCapabilities) {
    if opts.channel {
        caps.experimental
            .get_or_insert_default()
            .insert(CHANNEL_CAPABILITY.to_owned(), JsonObject::new());
    }
}

/// The `notifications/claude/channel` for one wake: the wake text as `content`
/// and string-valued `meta` (Claude Code turns each key into an attribute of
/// the `<channel>` tag; keys must be identifiers).
pub fn notification(wake: &Wake) -> ServerNotification {
    ServerNotification::CustomNotification(CustomNotification::new(
        CHANNEL_METHOD,
        Some(json!({
            "content": wake.text,
            "meta": {
                "review_id": wake.review_id,
                "submission_id": wake.submission_id,
                "verdict": wake.verdict.as_str(),
            },
        })),
    ))
}

/// The submissions to push for one session: an [`EventFeed`] of
/// `review.submitted` from the moment it opened, filtered by
/// [`wake::wakes_for`]. Blocking; `Send`, so it can move to the blocking pool.
pub struct SubmissionWatch {
    core: Core,
    session_id: String,
    feed: EventFeed,
}

impl SubmissionWatch {
    /// Opens the watch at the latest event: only later submissions are reported.
    pub fn open(core: &Core, session_id: &str) -> Result<SubmissionWatch, CoreError> {
        let mut feed = EventFeed::open(
            &core.paths,
            0,
            EventFilter {
                kinds: Some(vec![EventKind::ReviewSubmitted]),
                ..EventFilter::default()
            },
        )?;
        feed.seek_to_latest()?;
        Ok(SubmissionWatch {
            core: core.clone(),
            session_id: session_id.to_owned(),
            feed,
        })
    }

    /// The wakes among the submissions since the last poll, in `seq` order.
    pub fn poll(&mut self) -> Result<Vec<Wake>, CoreError> {
        let events = self.feed.poll()?;
        if events.is_empty() {
            return Ok(Vec::new());
        }
        wake::wakes_for(&self.core, &self.session_id, &events)
    }
}

/// Called once the client is initialized: with `opts.channel`, watch the event
/// feed for submissions of reviews assigned to `session_id` and push one
/// `notifications/claude/channel` each through `peer`. Must run inside the
/// tokio runtime; returns at once.
pub fn start(opts: &ServeOptions, core: &Core, session_id: &str, peer: &Peer<RoleServer>) {
    if !opts.channel {
        return;
    }
    let core = core.clone();
    let session_id = session_id.to_owned();
    let peer = peer.clone();
    tokio::spawn(run(core, session_id, peer));
}

/// The push loop: poll on the blocking pool, send on the transport.
async fn run(core: Core, session_id: String, peer: Peer<RoleServer>) {
    let opened =
        tokio::task::spawn_blocking(move || SubmissionWatch::open(&core, &session_id)).await;
    let mut watch = match opened {
        Ok(Ok(w)) => w,
        Ok(Err(e)) => {
            tracing::warn!("claude/channel: opening the event feed failed: {e}");
            return;
        }
        Err(e) => {
            tracing::warn!("claude/channel: opening the event feed panicked: {e}");
            return;
        }
    };
    let mut ticks = tokio::time::interval(POLL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failing = false;
    loop {
        ticks.tick().await;
        if peer.is_transport_closed() {
            return;
        }
        let polled = tokio::task::spawn_blocking(move || {
            let r = watch.poll();
            (watch, r)
        })
        .await;
        let (w, wakes) = match polled {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("claude/channel: polling panicked: {e}");
                return;
            }
        };
        watch = w;
        let wakes = match wakes {
            Ok(wakes) => {
                failing = false;
                wakes
            }
            Err(e) => {
                if failing {
                    tracing::debug!("claude/channel: polling failed: {e}");
                } else {
                    tracing::warn!("claude/channel: polling failed: {e}");
                }
                failing = true;
                continue;
            }
        };
        for wake in &wakes {
            if let Err(e) = peer.send_notification(notification(wake)).await {
                tracing::debug!("claude/channel: sending failed, stopping: {e}");
                return;
            }
            tracing::debug!(
                review_id = wake.review_id,
                submission_id = wake.submission_id,
                "claude/channel: pushed a submission"
            );
        }
    }
}
