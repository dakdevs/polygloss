//! Automatic pruning of stale reviews (design §11.2, OQ-34): when
//! `storage.prune_reviews_after_days` is set, `Core::prune_stale` runs at
//! launch and then every [`PRUNE_INTERVAL`], on the background executor.
//! Core keeps orphaned reviews, reviews with drafts and reviews awaiting
//! you, and checks again inside each delete transaction.
//!
//! [`AutoPrune`] records the runs; Home observes it and refreshes after a
//! run that pruned something.

use std::time::Duration;

use gpui_kit::{App, AsyncApp, BorrowAppContext as _, Global};
use polygloss_core::store::events::now_ms;

use crate::app_state::AppState;
use crate::settings::SettingsStore;

/// How often stale reviews are pruned after the launch run.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// What automatic pruning did so far (a GPUI global).
#[derive(Debug, Default)]
pub struct AutoPrune {
    /// Runs made (only while the setting is set).
    pub runs: u32,
    /// Every review id pruned, oldest run first.
    pub pruned: Vec<String>,
}

impl Global for AutoPrune {}

/// Starts the launch run and the 24-hour schedule. Each run reads the
/// setting then, so a later settings change applies from the next run.
pub fn start(cx: &mut App) {
    cx.set_global(AutoPrune::default());
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            run_once(cx).await;
            cx.background_executor().timer(PRUNE_INTERVAL).await;
        }
    })
    .detach();
}

/// One run: prunes when the setting is set, then records it.
async fn run_once(cx: &mut AsyncApp) {
    let (days, core) = cx.update(|cx| {
        (
            SettingsStore::global(cx)
                .settings()
                .storage
                .prune_reviews_after_days,
            AppState::global(cx).core.clone(),
        )
    });
    let Some(days) = days else {
        return;
    };
    let result = cx
        .background_executor()
        .spawn(async move { core.prune_stale(days, now_ms()) })
        .await;
    let pruned = match result {
        Ok(pruned) => pruned,
        Err(e) => {
            tracing::warn!("pruning reviews older than {days} days: {e}");
            Vec::new()
        }
    };
    if !pruned.is_empty() {
        tracing::info!("pruned {} stale reviews", pruned.len());
    }
    cx.update(|cx| {
        cx.update_global::<AutoPrune, _>(|state, _| {
            state.runs += 1;
            state.pruned.extend(pruned);
        })
    });
}
