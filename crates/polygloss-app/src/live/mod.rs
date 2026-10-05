//! Live mode (design §10, ADR-0008, ADR-0009): the watcher, the banner, the
//! refresh, the base picker, Snapshot, and compare reviews watching their
//! refs (OQ-27).
//!
//! - [`watcher`]: [`LiveWatcher`], `notify` through `notify-debouncer-full`
//!   on the worktree and the git dirs, filtered on its thread.
//! - [`recompute`]: after each relevant batch, an unpinned snapshot +
//!   `diff-tree` + blob-pair compare in the background → the banner
//!   "N files changed · Refresh (R)" ("Base moved · …" when the base tree
//!   moved), or "New iteration available · Refresh (R)" for a compare
//!   review whose refs moved. Nothing on screen changes (ADR-0009).
//! - [`refresh`]: `tab::Refresh` (`R`, the banner's button) swaps the new
//!   state in, keeping the scroll anchor by line mapping, collapsed files,
//!   revealed context and Viewed; [`refresh::DiffRefreshed`] tells the
//!   tab's features.
//! - [`base_picker`]: `tab::ChooseBase` (and the toolbar's Live pill):
//!   merge base (default), HEAD or a fixed commit, each its own review key
//!   in its own tab.
//! - `tab::Snapshot` pins the live state shown (`pinned_by = manual`).
//!
//! A tab's state is the [`Live`] extension. Watchers start after the tab's
//! first frame (so they never cost first paint) and keep running while the
//! app is unfocused; they stop when the tab closes.

pub mod base_picker;
pub mod recompute;
pub mod refresh;
pub mod watcher;

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncWindowContext, Context, Entity, InteractiveElement as _,
    IntoElement as _, MenuItem, SharedString, Subscription, WeakEntity, Window,
};
use polygloss_core::git::{ReviewKind, Since};
use polygloss_core::review::{OpenRequest, OpenedDiff, PinnedBy};
use polygloss_core::store::events::Actor;

pub use recompute::{Newer, Outcome, Target};
pub use refresh::DiffRefreshed;
pub use watcher::{LiveWatcher, WatcherChanged};

use crate::app_state::AppState;
use crate::keymap::actions::tab as tab_actions;
use crate::keymap::handlers;
use crate::provider::CoreDiffProvider;
use crate::review_tab::toolbar::{self, Narrow};
use crate::review_tab::{BannerKind, ReviewTab};
use crate::window::MenuKind;

/// Retries after the user's `index.lock` blocked a snapshot, one per
/// debounce period (design §5.3), before waiting for the next change.
const MAX_LOCKED_RETRIES: u32 = 25;

/// Registers the actions (`tab::Refresh`, `tab::Snapshot`,
/// `tab::ChooseBase`; their keys are in `keymap::defaults`) and the Review
/// menu items.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::Refresh, window, cx| refresh_tab(tab, window, cx),
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::Snapshot, window, cx| snapshot(tab, window, cx),
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::ChooseBase, window, cx| {
            base_picker::open(tab, window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::Review,
        vec![
            MenuItem::action("Refresh", tab_actions::Refresh),
            MenuItem::action("Snapshot", tab_actions::Snapshot),
            MenuItem::action("Choose Base…", tab_actions::ChooseBase),
        ],
        cx,
    );
}

/// A review tab's live state (a [`ReviewTab`] extension). Commit reviews
/// have none: they never move.
pub struct Live {
    target: Target,
    watcher: Option<Entity<LiveWatcher>>,
    /// A recompute is running.
    running: bool,
    /// Changes arrived while a recompute or refresh ran: recompute again
    /// once it ends.
    again: bool,
    /// What the banner announces.
    newer: Option<Newer>,
    /// A refresh is running.
    refreshing: bool,
    locked_retries: u32,
    /// Recomputes finished so far.
    recomputes: u64,
    _subscription: Option<Subscription>,
}

impl Live {
    /// What the tab recomputes.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The watcher, once the tab drew its first frame.
    pub fn watcher(&self) -> Option<&Entity<LiveWatcher>> {
        self.watcher.as_ref()
    }

    /// The newer state the banner announces, if any.
    pub fn newer(&self) -> Option<&Newer> {
        self.newer.as_ref()
    }

    /// Recomputes finished so far.
    pub fn recomputes(&self) -> u64 {
        self.recomputes
    }

    /// Whether a recompute or a refresh is running.
    pub fn busy(&self) -> bool {
        self.running || self.refreshing
    }

    /// Recomputes since the last one that found the index locked (0 when
    /// the last one was not blocked).
    pub fn locked_retries(&self) -> u32 {
        self.locked_retries
    }
}

/// The live state of `tab` (`None` for a commit review).
pub fn live(tab: &ReviewTab) -> Option<&Live> {
    tab.extension::<Live>()
}

/// Sets live mode up on a new tab: the [`Live`] state now, the watcher
/// after the tab's first frame.
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(target) = Target::of(&tab.opened) else {
        return;
    };
    tab.insert_extension(Live {
        target,
        watcher: None,
        running: false,
        again: false,
        newer: None,
        refreshing: false,
        locked_retries: 0,
        recomputes: 0,
        _subscription: None,
    });
    let this = cx.entity().downgrade();
    window.on_next_frame(move |_, cx| {
        if let Some(tab) = this.upgrade() {
            tab.update(cx, start_watcher);
        }
    });
}

fn start_watcher(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(live) = tab.extension::<Live>() else {
        return;
    };
    if live.watcher.is_some() {
        return;
    }
    let target = live.target.clone();
    let watcher = match target.kind {
        ReviewKind::Live => LiveWatcher::start(&target.repo, &target.worktree, cx),
        _ => LiveWatcher::start_refs(&target.repo, cx),
    };
    let subscription = cx.subscribe(&watcher, |tab, _, _: &WatcherChanged, cx| {
        recompute_now(tab, cx);
    });
    if let Some(live) = tab.extension_mut::<Live>() {
        live.watcher = Some(watcher);
        live._subscription = Some(subscription);
    }
}

/// Recomputes the tab's source in the background and updates the banner
/// (what the watcher does after each batch). Serialized: while one runs,
/// another is queued.
pub fn recompute_now(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    // The tab's current state, even while it shows an earlier iteration or
    // the changes since the last review (T3.12).
    let shown = recompute::Shown::of(crate::iterations::current(tab));
    let Some(live) = tab.extension_mut::<Live>() else {
        return;
    };
    if live.running || live.refreshing {
        live.again = true;
        return;
    }
    live.running = true;
    let target = live.target.clone();
    let snapshots = AppState::global(cx).core.snapshots.clone();
    cx.spawn(async move |tab: WeakEntity<ReviewTab>, cx| {
        let outcome = cx
            .background_spawn(async move { recompute::recompute(&snapshots, &target, &shown) })
            .await;
        let _ = tab.update(cx, |tab, cx| recomputed(tab, outcome, cx));
    })
    .detach();
}

fn recomputed(tab: &mut ReviewTab, outcome: anyhow::Result<Outcome>, cx: &mut Context<ReviewTab>) {
    let Some(live) = tab.extension_mut::<Live>() else {
        return;
    };
    live.running = false;
    live.recomputes += 1;
    match outcome {
        Ok(Outcome::Same) => {
            live.locked_retries = 0;
            live.newer = None;
        }
        Ok(Outcome::Newer(newer)) => {
            live.locked_retries = 0;
            live.newer = Some(newer);
        }
        Ok(Outcome::IndexLocked) => {
            if live.locked_retries < MAX_LOCKED_RETRIES {
                live.locked_retries += 1;
                cx.spawn(async move |tab: WeakEntity<ReviewTab>, cx| {
                    cx.background_executor().timer(watcher::DEBOUNCE).await;
                    let _ = tab.update(cx, recompute_now);
                })
                .detach();
            }
        }
        Err(e) => tracing::warn!("recomputing {}: {e:#}", tab.opened.review_key),
    }
    show_banner(tab, cx);
    let again = tab
        .extension_mut::<Live>()
        .is_some_and(|l| std::mem::take(&mut l.again));
    if again {
        recompute_now(tab, cx);
    }
}

/// The banner kind of a tab's changes.
fn banner_kind(kind: ReviewKind) -> BannerKind {
    match kind {
        ReviewKind::Compare => BannerKind::NewIteration,
        _ => BannerKind::LiveChanges,
    }
}

/// Shows (or hides) the tab's banner for what [`Live::newer`] holds.
fn show_banner(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(live) = tab.extension::<Live>() else {
        return;
    };
    let kind = banner_kind(live.target.kind);
    let text = live
        .newer
        .as_ref()
        .map(|n| SharedString::from(recompute::banner_text(live.target.kind, n)));
    tab.banners.update(cx, |b, cx| match text {
        Some(text) => b.set(kind, text, Box::new(tab_actions::Refresh), cx),
        None => b.clear(kind, cx),
    });
}

/// An open request named a review this tab already shows, and the open
/// produced another diff (the refs moved, or the worktree changed): the
/// tab keeps what it shows and offers the refresh (never auto-applied,
/// ADR-0009).
pub fn newer_diff_opened(tab: &mut ReviewTab, opened: &OpenedDiff, cx: &mut Context<ReviewTab>) {
    let current = crate::iterations::current(tab);
    if opened.diff_id == current.diff_id {
        return;
    }
    let newer = Newer {
        diff_id: opened.diff_id.clone(),
        files_changed: recompute::changed_files(&current.files, &opened.files),
        base_moved: opened.base.tree != current.base.tree,
        other_review: None,
    };
    let Some(live) = tab.extension_mut::<Live>() else {
        return;
    };
    live.newer = Some(newer);
    show_banner(tab, cx);
}

/// `tab::Refresh`: opens the tab's source again and swaps the new state in
/// ([`refresh`]). A live worktree now on another branch opens that review
/// in its own tab instead.
pub fn refresh_tab(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(live) = tab.extension_mut::<Live>() else {
        return;
    };
    if live.refreshing {
        return;
    }
    live.refreshing = true;
    let target = live.target.clone();
    let req = OpenRequest {
        worktree: target.worktree.clone(),
        source: target.source.clone(),
        label: None,
        pin: (target.kind == ReviewKind::Compare).then_some(PinnedBy::Refresh),
        actor: Actor::human(),
    };
    let kept = refresh::Kept::read(tab.viewport.read(cx));
    let old_files = tab.opened.files.clone();
    let old_provider = tab.viewport.read(cx).provider().clone();
    let review_id = tab.review_id.clone();
    let core = AppState::global(cx).core.clone();
    cx.spawn_in(window, async move |tab, cx: &mut AsyncWindowContext| {
        let prepared = cx
            .background_spawn(async move {
                let opened = core.open(&req)?;
                let provider = CoreDiffProvider::open(&opened)?;
                let plan = (opened.review_id == review_id).then(|| {
                    refresh::plan(&kept, &old_files, &opened.files, &*old_provider, &provider)
                });
                anyhow::Ok((opened, provider, plan))
            })
            .await;
        let _ = tab.update_in(cx, |tab, window, cx| refreshed(tab, prepared, window, cx));
    })
    .detach();
}

type Prepared = anyhow::Result<(OpenedDiff, CoreDiffProvider, Option<refresh::Plan>)>;

fn refreshed(
    tab: &mut ReviewTab,
    prepared: Prepared,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    if let Some(live) = tab.extension_mut::<Live>() {
        live.refreshing = false;
    }
    match prepared {
        Err(err) => {
            let message = format!("Could not refresh: {err:#}");
            tracing::warn!("{}: {message}", tab.opened.review_key);
            toast(message, true, window, cx);
        }
        Ok((opened, provider, None)) => {
            // The worktree moved to another branch: that is another review.
            clear_newer(tab, cx);
            if let Some((_, main)) = crate::window::main_window(cx) {
                main.update(cx, |main, cx| {
                    main.show_review(opened, Arc::new(provider), window, cx)
                });
            }
        }
        Ok((opened, provider, Some(plan))) => {
            let old = std::mem::replace(&mut tab.opened, opened);
            clear_newer(tab, cx);
            let context = crate::review_tab::description(&tab.opened);
            tab.banners
                .update(cx, |b, cx| b.set_context(context.into(), cx));
            // The new state is the tab's current one (T3.12).
            crate::iterations::refreshed(tab, cx);
            if old.diff_id != tab.opened.diff_id {
                refresh::apply(tab, old.diff_id, Arc::new(provider), plan, cx);
            }
            cx.notify();
        }
    }
    let again = tab
        .extension_mut::<Live>()
        .is_some_and(|l| std::mem::take(&mut l.again));
    if again {
        recompute_now(tab, cx);
    }
}

fn clear_newer(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    if let Some(live) = tab.extension_mut::<Live>() {
        live.newer = None;
    }
    show_banner(tab, cx);
}

/// `tab::Snapshot`: pins the live state shown as an iteration
/// (`pinned_by = manual`, design §5.2), on its displayed base.
pub fn snapshot(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    // The current live state, also while the tab shows an earlier
    // iteration (T3.12).
    let current = crate::iterations::current(tab);
    let Some(state) = current.live.clone() else {
        return;
    };
    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let base = current.base.clone();
    cx.spawn_in(window, async move |tab, cx: &mut AsyncWindowContext| {
        let pinned = cx
            .background_spawn(async move {
                core.pin_live_on_base(&review_id, &base, &state, PinnedBy::Manual, &Actor::human())
            })
            .await;
        let _ = tab.update_in(cx, |tab, window, cx| match pinned {
            Ok(it) => {
                let message = format!("Snapshot saved as iteration {}", it.seq);
                crate::iterations::pinned(tab, it, cx);
                toast(message, false, window, cx);
                cx.notify();
            }
            Err(err) => toast(
                format!("Could not save a snapshot: {err}"),
                true,
                window,
                cx,
            ),
        });
    })
    .detach();
}

/// Shows `message` in the main window (an error or a confirmation).
fn toast(message: String, error: bool, window: &mut Window, cx: &mut App) {
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    main.update(cx, |main, cx| {
        if error {
            main.toast_error(message.into(), window, cx);
        } else {
            main.toast_success(message.into(), window, cx);
        }
    });
}

/// The base a live review shows, from its key.
pub fn since(tab: &ReviewTab) -> Option<Since> {
    (tab.opened.kind == ReviewKind::Live)
        .then(|| recompute::since_of(&tab.opened.review_key))
        .flatten()
}

/// The toolbar's live controls (design §11.4), for live reviews: the Live
/// pill ("Live · merge base"), which opens the base picker, then Snapshot
/// (until the header card takes it, T6.13).
pub fn toolbar_left(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    let Some(since) = since(tab) else {
        return Vec::new();
    };
    let current = crate::iterations::current(tab);
    let pinned = current
        .iteration
        .as_ref()
        .filter(|it| it.diff_id == current.diff_id)
        .map(|it| it.seq);
    let narrow = toolbar::narrow(tab);
    let text = format!("Live \u{b7} {}", base_picker::since_label(&since));
    let icon_only = narrow >= Narrow::IconPills;
    let live = toolbar::pill(
        "live-base",
        Lucide::CircleDot,
        (!icon_only).then(|| toolbar::pill_text("live-base-label", text.clone())),
        cx,
    )
    .tooltip(if icon_only {
        text
    } else {
        "Choose the base of the working tree diff".to_owned()
    })
    .on_click(cx.listener(|tab, _, window, cx| {
        base_picker::open(tab, window, cx);
    }));
    let snapshot = Button::new("live-snapshot")
        .debug_selector(|| "live-snapshot".into())
        .label("Snapshot")
        .small()
        .ghost()
        .disabled(pinned.is_some())
        .tooltip(match pinned {
            Some(seq) => format!("Saved as iteration {seq}"),
            None => "Pin this state as an iteration".to_owned(),
        })
        .on_click(cx.listener(|tab, _, window, cx| snapshot(tab, window, cx)));
    vec![live.into_any_element(), snapshot.into_any_element()]
}
