//! The store feed in a review tab (design §8.3 step 5, §11.7, §17): the
//! "claude-code replied to N threads" banner (threads with agent events
//! after `reviews.last_seen_seq`; its button jumps to the next unread
//! thread and marks it seen) and the "Re-review requested" banner (the
//! agent's summary, "View changes" = Changes since last review, T3.12).

use std::collections::BTreeSet;

use gpui_kit::{AppContext as _, Context, SharedString, Task, Window};
use polygloss_core::review::{Rereview, ReviewActivity, UnreadThread};
use polygloss_core::store::events::{ActorKind, Event, EventKind};

use crate::app_state::AppState;
use crate::keymap::actions::tab as tab_actions;
use crate::review_tab::{BannerKind, ReviewTab};
use crate::threads::{self, placement::ThreadPlace};

/// The longest re-review summary line the banner shows.
const SUMMARY_CHARS: usize = 160;

/// A review tab's view of the feed (a [`ReviewTab`] extension).
#[derive(Default)]
pub struct TabFeed {
    /// The review's agent activity as last read; `None` until the first
    /// read lands.
    activity: Option<ReviewActivity>,
    /// Bumped by every read and jump; a read that finishes under an older
    /// generation is dropped.
    generation: u64,
    loading: Option<Task<()>>,
    /// The latest `mark_seen` write and the read after it.
    marking: Option<Task<()>>,
}

/// The review's agent activity as the tab last read it.
pub fn activity(tab: &ReviewTab) -> Option<&ReviewActivity> {
    tab.extension::<TabFeed>()?.activity.as_ref()
}

pub(crate) fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    tab.insert_extension(TabFeed::default());
    load(tab, cx);
}

/// Whether `kind` is about a thread or its comments.
fn thread_kind(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::ThreadCreated
            | EventKind::CommentCreated
            | EventKind::CommentEdited
            | EventKind::CommentDeleted
            | EventKind::ThreadResolved
            | EventKind::ThreadUnresolved
    )
}

/// New store events: threads of this review (or shown in this diff)
/// reload, and agent activity, re-review requests and submissions re-read
/// the banners' state.
pub(crate) fn on_events(tab: &mut ReviewTab, events: &[Event], cx: &mut Context<ReviewTab>) {
    let ours = |e: &&Event| e.review_id.as_deref() == Some(tab.review_id.as_str());
    let diff = tab.opened.diff_id.as_str();
    let threads_changed = events
        .iter()
        .any(|e| thread_kind(e.kind) && (ours(&e) || e.diff_id.as_deref() == Some(diff)));
    let activity_changed = events.iter().filter(ours).any(|e| {
        (thread_kind(e.kind) && e.actor.kind == ActorKind::Agent)
            || matches!(
                e.kind,
                EventKind::ReviewRereviewRequested | EventKind::ReviewSubmitted
            )
    });
    if threads_changed {
        threads::reload(tab, cx);
    }
    if activity_changed {
        load(tab, cx);
    }
}

/// Reads the review's activity on the background executor, then shows it.
fn load(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(state) = tab.extension_mut::<TabFeed>() else {
        return;
    };
    state.generation += 1;
    let generation = state.generation;
    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let read = cx.background_spawn(async move { core.review_activity(&review_id) });
    let task = cx.spawn(async move |this, cx| {
        let activity = read.await;
        this.update(cx, |tab, cx| {
            let current = tab
                .extension::<TabFeed>()
                .is_some_and(|s| s.generation == generation);
            if !current {
                return;
            }
            match activity {
                Ok(activity) => {
                    if let Some(state) = tab.extension_mut::<TabFeed>() {
                        state.activity = Some(activity);
                        state.loading = None;
                    }
                    show_banners(tab, cx);
                }
                Err(e) => tracing::warn!("reading the activity of {}: {e}", tab.review_id),
            }
        })
        .ok();
    });
    if let Some(state) = tab.extension_mut::<TabFeed>() {
        state.loading = Some(task);
    }
}

/// "claude-code replied to 2 threads" (the agent of the latest event of
/// each thread; "Agents" when they differ); `None` when nothing is unread.
pub fn replies_text(unread: &[UnreadThread]) -> Option<String> {
    let names: BTreeSet<&str> = unread
        .iter()
        .map(|u| u.actor_name.as_deref().unwrap_or("An agent"))
        .collect();
    let who = match names.len() {
        0 => return None,
        1 => names.first().copied().unwrap_or("An agent"),
        _ => "Agents",
    };
    let n = unread.len();
    Some(format!(
        "{who} replied to {n} thread{}",
        if n == 1 { "" } else { "s" }
    ))
}

/// "claude-code requested a re-review: <the summary's first line as plain
/// text>".
pub fn rereview_text(r: &Rereview) -> String {
    let who = r.requested_by.as_deref().unwrap_or("The agent");
    match summary_line(&r.summary) {
        Some(line) => format!("{who} requested a re-review: {line}"),
        None => format!("{who} requested a re-review"),
    }
}

/// The first block of a markdown summary as plain text (formatting marks
/// dropped), cut to [`SUMMARY_CHARS`] (the banner and T3.17's notification).
pub fn summary_line(summary: &str) -> Option<String> {
    let plain = std::panic::catch_unwind(|| {
        let root = markdown::to_mdast(summary, &markdown::ParseOptions::gfm()).ok()?;
        root.children()?
            .iter()
            .map(|n| n.to_string())
            .find(|t| !t.trim().is_empty())
    })
    .ok()
    .flatten()
    // A body the parser cannot take: its first non-empty line as written.
    .or_else(|| {
        summary
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(str::to_owned)
    })?;
    let line = plain.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= SUMMARY_CHARS {
        return Some(line.to_owned());
    }
    let cut: String = line.chars().take(SUMMARY_CHARS - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// Sets or clears the two banners from the tab's activity.
fn show_banners(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(activity) = activity(tab) else {
        return;
    };
    let replies = replies_text(&activity.unread);
    let rereview = activity.rereview.as_ref().map(rereview_text);
    tab.banners.update(cx, |b, cx| {
        match replies {
            Some(text) => b.set(
                BannerKind::AgentReplies,
                SharedString::from(text),
                Box::new(tab_actions::NextUnreadThread),
                cx,
            ),
            None => b.clear(BannerKind::AgentReplies, cx),
        }
        match rereview {
            Some(text) => b.set(
                BannerKind::Rereview,
                SharedString::from(text),
                Box::new(tab_actions::ToggleChangesSinceLastReview),
                cx,
            ),
            None => b.clear(BannerKind::Rereview, cx),
        }
    });
}

/// "Show" on the replies banner (`tab::NextUnreadThread`): the next unread
/// thread (the one the agent touched longest ago) is shown and the review
/// is marked seen up to its latest event, so the count drops by one.
pub fn next_unread(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(state) = tab.extension_mut::<TabFeed>() else {
        return;
    };
    let Some(activity) = state.activity.as_mut() else {
        return;
    };
    if activity.unread.is_empty() {
        return;
    }
    let next = activity.unread.remove(0);
    activity.last_seen_seq = activity.last_seen_seq.max(next.last_seq);
    // A read in flight predates this jump.
    state.generation += 1;
    state.loading = None;
    show_banners(tab, cx);
    show_thread(tab, &next.thread_id, window, cx);

    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let seq = next.last_seq;
    let write = cx.background_spawn(async move { core.mark_seen(&review_id, seq) });
    let task = cx.spawn(async move |this, cx| {
        let written = write.await;
        this.update(cx, |tab, cx| {
            if let Err(e) = written {
                tracing::warn!("marking {} seen: {e}", tab.review_id);
            }
            load(tab, cx);
        })
        .ok();
    });
    if let Some(state) = tab.extension_mut::<TabFeed>() {
        state.marking = Some(task);
    }
}

/// Shows thread `id`: in the diff (the cursor on it, like a click in the
/// threads panel), or opened in the threads panel when the diff cannot show
/// it (review threads, files not in this diff).
fn show_thread(tab: &mut ReviewTab, id: &str, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads::threads(tab).cloned() else {
        return;
    };
    let (known, in_diff, expanded) = {
        let m = model.read(cx);
        let shows = m.thread(id).is_some_and(|t| m.shows(t));
        let placed = m.place(id).is_some_and(|p| *p != ThreadPlace::Panel);
        (m.thread(id).is_some(), shows && placed, m.is_expanded(id))
    };
    if !known {
        return;
    }
    if in_diff {
        threads::activate_thread(tab, id, window, cx);
        return;
    }
    if !tab.threads_panel_visible() {
        tab.toggle_threads_panel(cx);
    }
    // In the panel a click opens or closes the card: open it only.
    if !expanded {
        threads::activate_thread(tab, id, window, cx);
    }
}
