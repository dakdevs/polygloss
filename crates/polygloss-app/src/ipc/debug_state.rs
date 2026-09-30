//! The test-only `debug_state` op (plan OQ-P4; the core server answers it
//! only with `POLYGLOSS_TEST=1`): what bun E2E suites check in a running
//! app.
//!
//! ```text
//! { window_open, app_active,
//!   tabs: [{ review_id, diff_id, title, active, anchor, cursor }],
//!   focused_tab: <review_id of the active tab> | null,
//!   banners: [{ review_id, kind, text }],
//!   badge: <Dock badge count>, events_seen: <store events the feed handed out>,
//!   feed_polls: <polls the feed finished>, feed_errors: <polls that failed>,
//!   activations: <times the app asked macOS to activate it> }
//! ```
//!
//! `anchor` is the line at the top of the viewport and `cursor` the line
//! cursor, both `{path, side, line}` with 1-based lines (or `null`).

use gpui_kit::App;
use polygloss_diff::{FileChange, Side};
use serde_json::{Value, json};

use crate::feed::StoreFeed;
use crate::review_tab::BannerKind;
use crate::window::main_window;

/// The app's state as JSON (module docs).
pub fn snapshot(cx: &App) -> Value {
    let badge = crate::notify::badge_count(cx);
    let stats = cx
        .try_global::<StoreFeed>()
        .map(StoreFeed::stats)
        .unwrap_or_default();
    let events_seen = stats.events;
    let activations = crate::window::activation_requests(cx);
    let app_active = cx.active_window().is_some();
    let Some((_, main)) = main_window(cx) else {
        return json!({
            "window_open": false,
            "app_active": app_active,
            "tabs": [],
            "focused_tab": null,
            "banners": [],
            "badge": badge,
            "events_seen": events_seen,
            "feed_polls": stats.polls,
            "feed_errors": stats.errors,
            "activations": activations,
        });
    };
    let main = main.read(cx);
    let active = main.tabs().active();
    let mut tabs = Vec::new();
    let mut banners = Vec::new();
    let mut focused = Value::Null;
    for (ix, item) in main.tabs().items().iter().enumerate() {
        let Some(tab) = item.review() else {
            continue;
        };
        let t = tab.read(cx);
        let viewport = t.viewport.read(cx);
        let files = viewport.document().files();
        let anchor = crate::view_state::top_line(viewport)
            .map(|(f, side, line)| location(files, f, side, line));
        let cursor = viewport
            .cursor()
            .map(|c| location(files, c.file_idx, c.side, c.line));
        for (kind, text) in t.banners.read(cx).banners() {
            banners.push(json!({
                "review_id": t.review_id,
                "kind": banner_name(kind),
                "text": text.to_string(),
            }));
        }
        if ix == active {
            focused = json!(t.review_id);
        }
        tabs.push(json!({
            "review_id": t.review_id,
            "diff_id": t.opened.diff_id.as_str(),
            "title": t.title().to_string(),
            "active": ix == active,
            "anchor": anchor,
            "cursor": cursor,
        }));
    }
    json!({
        "window_open": true,
        "app_active": app_active,
        "tabs": tabs,
        "focused_tab": focused,
        "banners": banners,
        "badge": badge,
        "events_seen": events_seen,
        "feed_polls": stats.polls,
        "feed_errors": stats.errors,
        "activations": activations,
    })
}

/// `{path, side, line}` for 0-based `line` of file `file_idx`.
fn location(files: &[FileChange], file_idx: u32, side: Side, line: u32) -> Value {
    json!({
        "path": files.get(file_idx as usize).map(FileChange::display_path),
        "side": side,
        "line": line + 1,
    })
}

/// A banner's wire name.
pub fn banner_name(kind: BannerKind) -> &'static str {
    match kind {
        BannerKind::LiveChanges => "live_changes",
        BannerKind::NewIteration => "new_iteration",
        BannerKind::AgentReplies => "agent_replies",
        BannerKind::Rereview => "rereview",
    }
}
