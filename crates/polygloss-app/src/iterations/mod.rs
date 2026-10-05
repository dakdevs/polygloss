//! Iterations and "Changes since last review" (design §11.4, §8.6, OQ-9).
//!
//! A review tab keeps its **current** state (the latest iteration of a
//! commit or compare review; the live state, pinned or not, of a live
//! review; a refresh replaces it) and shows one of ([`Showing`]):
//!
//! - the current state;
//! - an earlier iteration, picked from the toolbar's "Iteration k of n"
//!   picker ([`show`]; `Core::open_iteration` reads it from the store);
//! - **Changes since last review** ([`toggle_changes_since`],
//!   `tab::ToggleChangesSinceLastReview`, also in the picker's menu): the
//!   pinned diff from the head tree of the iteration the last submission
//!   was made against to the current head (`Core::open_changes_since`).
//!   A live state that is not pinned yet is pinned first (`pinned_by =
//!   manual`, like Snapshot), so both ends are durable and comments made in
//!   this mode anchor to objects in the repo. Comments are allowed on the
//!   new side only (OQ-9, `DiffViewport::set_old_side_comments`); a rebased
//!   base shows up as noise until range-diff (post-v1).
//!
//! Every switch happens in the same tab and viewport, like a live refresh
//! (`live::refresh::{plan, apply}`): the scroll anchor is line-mapped,
//! collapsed files, revealed context and Viewed marks follow, and
//! [`crate::live::DiffRefreshed`] makes the tab's features follow (the tree
//! rebuilds, threads are placed again with carry-forward, Viewed reloads).
//! The live watcher keeps comparing with the current state, not with what
//! is shown, and a refresh brings the tab back to the (new) current state.
//!
//! The picker shows once there is something to pick: two or more states,
//! or a submission (commit reviews never move, so never).

pub mod picker;

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, AsyncWindowContext, Context, MenuItem, SharedString, Task, Window,
};
use polygloss_core::git::ReviewKind;
use polygloss_core::review::{IterationEntry, IterationInfo, LastSubmission, OpenedDiff, PinnedBy};
use polygloss_core::store::events::Actor;
use polygloss_diff::Oid;

pub use picker::toolbar_left;

use crate::app_state::AppState;
use crate::keyboard::menu::KeyMenu;
use crate::keymap::actions::tab as tab_actions;
use crate::keymap::handlers;
use crate::live::refresh;
use crate::provider::CoreDiffProvider;
use crate::review_tab::{ReviewTab, short_ref};
use crate::window::MenuKind;

/// What a review tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showing {
    /// The current state.
    Current,
    /// Earlier iteration `seq`.
    Iteration(u32),
    /// The changes since the last submission, made against iteration
    /// `since`.
    ChangesSince { since: u32 },
}

/// An entry of the iteration picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The current state (the latest iteration, or the working tree).
    Current,
    /// Iteration `seq`.
    Iteration(u32),
}

/// A row of the picker's menu, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerEntry {
    pub choice: Choice,
    /// "Iteration 3", "Working tree".
    pub label: String,
    /// "latest · a1b2c3d · refreshed".
    pub detail: String,
    /// When it was pinned (Unix ms); `None` for an unpinned working tree.
    pub at: Option<i64>,
    /// It is what the tab shows.
    pub selected: bool,
}

/// A tab's iteration state (a [`ReviewTab`] extension).
pub struct Iterations {
    current: OpenedDiff,
    showing: Showing,
    entries: Vec<IterationEntry>,
    last: Option<LastSubmission>,
    loaded: bool,
    /// A switch is running.
    switching: bool,
    generation: u64,
    load: Option<Task<()>>,
    /// The picker's menu, when opened from the keyboard (`i`).
    key_menu: Option<KeyMenu>,
}

impl Iterations {
    /// The tab's current state.
    pub fn current(&self) -> &OpenedDiff {
        &self.current
    }

    pub fn showing(&self) -> Showing {
        self.showing
    }

    /// The review's iterations in `seq` order, as last loaded.
    pub fn entries(&self) -> &[IterationEntry] {
        &self.entries
    }

    /// The review's last submission, as last loaded.
    pub fn last_submission(&self) -> Option<&LastSubmission> {
        self.last.as_ref()
    }

    /// The first load has landed.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// A switch is running.
    pub fn busy(&self) -> bool {
        self.switching
    }

    /// The current state's iteration, when it is pinned.
    fn current_seq(&self) -> Option<u32> {
        self.current
            .iteration
            .as_ref()
            .filter(|it| it.diff_id == self.current.diff_id)
            .map(|it| it.seq)
    }

    /// Iterations known (at least the current one's `seq`).
    fn count(&self) -> u32 {
        let n = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        n.max(self.current_seq().unwrap_or(0))
    }
}

/// Registers `tab::ToggleChangesSinceLastReview` and its Review menu item.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::ToggleChangesSinceLastReview, window, cx| {
            toggle_changes_since(tab, window, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::ChooseIteration, window, cx| {
            open_menu(tab, window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::Review,
        vec![MenuItem::action(
            "Changes Since Last Review",
            tab_actions::ToggleChangesSinceLastReview,
        )],
        cx,
    );
}

/// Sets iterations up on a new tab: the tab's state is its current one;
/// the review's iterations and last submission load in the background, and
/// again whenever its threads change (a comment pins a live state, a
/// submission publishes drafts).
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    tab.insert_extension(Iterations {
        current: tab.opened.clone(),
        showing: Showing::Current,
        entries: Vec::new(),
        last: None,
        loaded: false,
        switching: false,
        generation: 0,
        load: None,
        key_menu: None,
    });
    if let Some(model) = crate::threads::threads(tab).cloned() {
        cx.subscribe(
            &model,
            |tab: &mut ReviewTab, _, _: &crate::threads::ThreadsEvent, cx| reload(tab, cx),
        )
        .detach();
    }
    reload(tab, cx);
}

/// The tab's iteration state.
pub fn state(tab: &ReviewTab) -> Option<&Iterations> {
    tab.extension::<Iterations>()
}

/// The tab's current state (what it shows when [`Showing::Current`]):
/// what the live watcher compares with and Snapshot pins.
pub fn current(tab: &ReviewTab) -> &OpenedDiff {
    state(tab).map_or(&tab.opened, |s| &s.current)
}

/// What the tab shows.
pub fn showing(tab: &ReviewTab) -> Showing {
    state(tab).map_or(Showing::Current, |s| s.showing)
}

/// Loads the review's iterations and last submission again (background).
pub fn reload(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let Some(s) = tab.extension_mut::<Iterations>() else {
        return;
    };
    s.generation += 1;
    let generation = s.generation;
    s.load = Some(cx.spawn(async move |tab, cx| {
        let loaded = cx
            .background_spawn(async move {
                let entries = core.iteration_entries(&review_id)?;
                let last = core.last_submission(&review_id)?;
                anyhow::Ok((entries, last))
            })
            .await;
        tab.update(cx, |tab, cx| match loaded {
            Ok((entries, last)) => loaded_state(tab, generation, entries, last, cx),
            Err(e) => tracing::warn!("loading the iterations of {}: {e:#}", tab.review_id),
        })
        .ok();
    }));
}

fn loaded_state(
    tab: &mut ReviewTab,
    generation: u64,
    entries: Vec<IterationEntry>,
    last: Option<LastSubmission>,
    cx: &mut Context<ReviewTab>,
) {
    let Some(s) = tab.extension_mut::<Iterations>() else {
        return;
    };
    if s.generation != generation {
        return;
    }
    // A live state pinned elsewhere (a comment, a submission, the agent):
    // the current state is that iteration now.
    let pinned = entries
        .iter()
        .rev()
        .find(|e| e.info.diff_id == s.current.diff_id)
        .map(|e| e.info.clone());
    if s.current_seq().is_none()
        && let Some(it) = pinned.clone()
    {
        s.current.iteration = Some(it);
    }
    s.entries = entries;
    s.last = last;
    s.loaded = true;
    s.load = None;
    let showing = s.showing;
    if showing == Showing::Current
        && tab.opened.iteration.is_none()
        && let Some(it) = pinned.filter(|it| it.diff_id == tab.opened.diff_id)
    {
        tab.opened.iteration = Some(it);
    }
    update_context(tab, cx);
    cx.notify();
}

/// A live refresh (`tab::Refresh`) replaced `tab.opened` with the new
/// current state: the tab shows it now. Called by `live` before it swaps
/// the new state into the viewport.
pub fn refreshed(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let opened = tab.opened.clone();
    let Some(s) = tab.extension_mut::<Iterations>() else {
        return;
    };
    s.current = opened;
    s.showing = Showing::Current;
    tab.viewport
        .update(cx, |v, cx| v.set_old_side_comments(true, cx));
    reload(tab, cx);
}

/// The current live state was pinned as `it` (Snapshot): the current state
/// and, when it shows it, the tab know their iteration.
pub fn pinned(tab: &mut ReviewTab, it: IterationInfo, cx: &mut Context<ReviewTab>) {
    if let Some(s) = tab.extension_mut::<Iterations>()
        && it.diff_id == s.current.diff_id
    {
        s.current.iteration = Some(it.clone());
    }
    if it.diff_id == tab.opened.diff_id && showing(tab) == Showing::Current {
        tab.opened.iteration = Some(it);
    }
    reload(tab, cx);
}

/// The picker's label: "Iteration k of n", "Working tree" (a live state not
/// pinned), or "Changes since last review".
pub fn label(tab: &ReviewTab) -> String {
    let Some(s) = state(tab) else {
        return String::new();
    };
    let n = s.count();
    match s.showing {
        Showing::ChangesSince { .. } => "Changes since last review".to_owned(),
        Showing::Iteration(k) => format!("Iteration {k} of {n}"),
        Showing::Current => match s.current_seq() {
            Some(k) => format!("Iteration {k} of {n}"),
            None => "Working tree".to_owned(),
        },
    }
}

/// Whether the iteration menu is open from the keyboard (`i`).
pub fn menu_open(tab: &ReviewTab) -> bool {
    state(tab).is_some_and(|s| s.key_menu.is_some())
}

/// `i`: the iteration picker's menu, opened from the keyboard under its
/// button (while the picker shows).
pub fn open_menu(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    if !picker_visible(tab) {
        return;
    }
    let data = picker::MenuData::of(tab);
    let weak = cx.entity().downgrade();
    let replacing = tab
        .extension_mut::<Iterations>()
        .and_then(|s| s.key_menu.take());
    let menu = KeyMenu::open(
        |t: &mut ReviewTab| t.extension_mut::<Iterations>().map(|s| &mut s.key_menu),
        replacing,
        move |menu, _, _| picker::build_menu(&weak, data, menu),
        window,
        cx,
    );
    if let Some(s) = tab.extension_mut::<Iterations>() {
        s.key_menu = Some(menu);
    }
    cx.notify();
}

/// Whether the toolbar shows the picker: a live or compare review with two
/// or more states to pick from or a submission, or one showing something
/// other than its current state.
pub fn picker_visible(tab: &ReviewTab) -> bool {
    let Some(s) = state(tab) else {
        return false;
    };
    if s.current.kind == ReviewKind::Commit {
        return false;
    }
    let states = s.entries.len() + usize::from(s.current_seq().is_none());
    states >= 2 || s.last.is_some() || s.showing != Showing::Current
}

/// The picker's rows, newest first: the working tree (a live state not
/// pinned), then every iteration. The iteration the current state is
/// pinned as stands for the current state.
pub fn picker_entries(tab: &ReviewTab) -> Vec<PickerEntry> {
    let Some(s) = state(tab) else {
        return Vec::new();
    };
    let current_seq = s.current_seq();
    let latest = s.entries.last().map(|e| e.info.seq);
    let mut out = Vec::with_capacity(s.entries.len() + 1);
    if current_seq.is_none() {
        out.push(PickerEntry {
            choice: Choice::Current,
            label: "Working tree".to_owned(),
            detail: "current · not saved as an iteration".to_owned(),
            at: None,
            selected: s.showing == Showing::Current,
        });
    }
    for e in s.entries.iter().rev() {
        let seq = e.info.seq;
        let choice = if Some(seq) == current_seq {
            Choice::Current
        } else {
            Choice::Iteration(seq)
        };
        let mut detail = Vec::new();
        if Some(seq) == latest {
            detail.push("latest".to_owned());
        }
        detail.push(head_name(e.head_commit.as_ref(), &e.head_tree));
        detail.push(pinned_by_label(e.pinned_by).to_owned());
        if s.last.as_ref().is_some_and(|l| l.iteration.info.seq == seq) {
            detail.push("last reviewed".to_owned());
        }
        out.push(PickerEntry {
            choice,
            label: format!("Iteration {seq}"),
            detail: detail.join(" · "),
            at: Some(e.created_at),
            selected: match choice {
                Choice::Current => s.showing == Showing::Current,
                Choice::Iteration(k) => s.showing == Showing::Iteration(k),
            },
        });
    }
    out
}

/// A head as the picker names it: the commit's short id, else the
/// snapshot's tree.
fn head_name(commit: Option<&Oid>, tree: &Oid) -> String {
    match commit {
        Some(c) => c.short().to_string(),
        None => format!("snapshot {}", tree.short()),
    }
}

/// Why an iteration was recorded, as the picker says it.
fn pinned_by_label(by: PinnedBy) -> &'static str {
    match by {
        PinnedBy::Open => "opened",
        PinnedBy::Refresh => "refreshed",
        PinnedBy::Comment => "saved by a comment",
        PinnedBy::Agent => "saved by the agent",
        PinnedBy::Manual => "snapshot",
        PinnedBy::Submit => "submitted",
        PinnedBy::Rereview => "re-review requested",
    }
}

/// Whether "Changes since last review" can be shown: there is a submission
/// and the head moved since.
pub fn changes_since_available(tab: &ReviewTab) -> bool {
    state(tab).is_some_and(|s| {
        s.last
            .as_ref()
            .is_some_and(|l| l.iteration.head_tree != s.current.head_tree)
    })
}

/// Whether "Changes since last review" is on.
pub fn changes_since_checked(tab: &ReviewTab) -> bool {
    matches!(showing(tab), Showing::ChangesSince { .. })
}

/// The toggle's tooltip: what it shows, or why it is off.
pub fn changes_since_hint(tab: &ReviewTab) -> String {
    let Some(s) = state(tab) else {
        return String::new();
    };
    match &s.last {
        None => "Submit a review first".to_owned(),
        Some(l) if l.iteration.head_tree == s.current.head_tree => {
            "Nothing changed since your last review".to_owned()
        }
        Some(l) => format!(
            "Show only what changed since you submitted iteration {}",
            l.iteration.info.seq
        ),
    }
}

/// Shows `choice` in the tab (same viewport; module docs).
pub fn show(tab: &mut ReviewTab, choice: Choice, window: &mut Window, cx: &mut Context<ReviewTab>) {
    // Nothing waits for it.
    drop(show_then(tab, choice, window, cx));
}

/// [`show`], and a future that resolves once the switch is done (shown or
/// failed; at once when there is nothing to switch or another switch is
/// running). The switch runs on whether or not the future is awaited.
pub fn show_then(
    tab: &mut ReviewTab,
    choice: Choice,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> impl Future<Output = ()> + use<> {
    let (done, finished) = futures::channel::oneshot::channel::<()>();
    let finished = async move {
        // Dropped unsent when nothing switched.
        let _ = finished.await;
    };
    let Some(s) = state(tab) else {
        return finished;
    };
    let target = match choice {
        Choice::Current => Showing::Current,
        Choice::Iteration(k) if Some(k) == s.current_seq() => Showing::Current,
        Choice::Iteration(k) => Showing::Iteration(k),
    };
    if s.switching || s.showing == target {
        return finished;
    }
    let current = s.current.clone();
    let core = AppState::global(cx).core.clone();
    switch(
        tab,
        target,
        move || match target {
            Showing::Iteration(k) => Ok(Switched {
                opened: core.open_iteration(&current, k)?,
                pinned: None,
            }),
            _ => Ok(Switched {
                opened: current,
                pinned: None,
            }),
        },
        Some(done),
        window,
        cx,
    );
    finished
}

/// `tab::ToggleChangesSinceLastReview`: shows the changes since the last
/// submission, or the current state again. Does nothing while
/// [`changes_since_available`] is false.
pub fn toggle_changes_since(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    if changes_since_checked(tab) {
        show(tab, Choice::Current, window, cx);
        return;
    }
    if !changes_since_available(tab) {
        return;
    }
    let Some(s) = state(tab) else {
        return;
    };
    let Some(since) = s.last.as_ref().map(|l| l.iteration.info.seq) else {
        return;
    };
    if s.switching {
        return;
    }
    let mut current = s.current.clone();
    let needs_pin = current.live.is_some() && s.current_seq().is_none();
    let core = AppState::global(cx).core.clone();
    switch(
        tab,
        Showing::ChangesSince { since },
        move || {
            // Both ends must be durable (module docs).
            let pinned = match (&current.live, needs_pin) {
                (Some(state), true) => {
                    let it = core.pin_live_on_base(
                        &current.review_id,
                        &current.base,
                        state,
                        PinnedBy::Manual,
                        &Actor::human(),
                    )?;
                    current.iteration = Some(it.clone());
                    Some(it)
                }
                _ => None,
            };
            let opened = core
                .open_changes_since(&current)?
                .ok_or_else(|| anyhow::anyhow!("the review has no submission"))?;
            Ok(Switched { opened, pinned })
        },
        None,
        window,
        cx,
    );
}

/// What a switch's background work produced: the diff to show, and the
/// iteration the current live state was pinned as on the way.
struct Switched {
    opened: OpenedDiff,
    pinned: Option<IterationInfo>,
}

/// Opens what `work` names off the main thread, then swaps it into the tab
/// like a refresh (module docs) and records `target`; `done` hears when it
/// is over (dropped unsent when the tab is gone).
fn switch(
    tab: &mut ReviewTab,
    target: Showing,
    work: impl FnOnce() -> anyhow::Result<Switched> + Send + 'static,
    done: Option<futures::channel::oneshot::Sender<()>>,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(s) = tab.extension_mut::<Iterations>() else {
        return;
    };
    s.switching = true;
    // The view state of the diff being left.
    crate::view_state::save_now(tab, cx);
    let kept = refresh::Kept::read(tab.viewport.read(cx));
    let old_files = tab.opened.files.clone();
    let old_provider = tab.viewport.read(cx).provider().clone();
    cx.spawn_in(window, async move |tab, cx: &mut AsyncWindowContext| {
        let prepared = cx
            .background_spawn(async move {
                let switched = work()?;
                let provider = CoreDiffProvider::open(&switched.opened)?;
                let plan = refresh::plan(
                    &kept,
                    &old_files,
                    &switched.opened.files,
                    &*old_provider,
                    &provider,
                );
                anyhow::Ok((switched, provider, plan))
            })
            .await;
        let _ = tab.update_in(cx, |tab, window, cx| {
            switched(tab, target, prepared, window, cx)
        });
        if let Some(done) = done {
            let _ = done.send(());
        }
    })
    .detach();
}

type Prepared = anyhow::Result<(Switched, CoreDiffProvider, refresh::Plan)>;

fn switched(
    tab: &mut ReviewTab,
    target: Showing,
    prepared: Prepared,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    if let Some(s) = tab.extension_mut::<Iterations>() {
        s.switching = false;
    }
    let (switched, provider, plan) = match prepared {
        Ok(p) => p,
        Err(err) => {
            let what = match target {
                Showing::ChangesSince { .. } => "the changes since your last review".to_owned(),
                Showing::Iteration(k) => format!("iteration {k}"),
                Showing::Current => "the latest state".to_owned(),
            };
            let message = format!("Could not show {what}: {err:#}");
            tracing::warn!("{}: {message}", tab.opened.review_key);
            if let Some((_, main)) = crate::window::main_window(cx) {
                main.update(cx, |main, cx| main.toast_error(message.into(), window, cx));
            }
            return;
        }
    };
    let Switched { opened, pinned } = switched;
    if let Some(s) = tab.extension_mut::<Iterations>() {
        if let Some(it) = pinned {
            s.current.iteration = Some(it);
        }
        s.showing = target;
    }
    let old = std::mem::replace(&mut tab.opened, opened);
    let changes_since = matches!(target, Showing::ChangesSince { .. });
    tab.viewport
        .update(cx, |v, cx| v.set_old_side_comments(!changes_since, cx));
    update_context(tab, cx);
    if old.diff_id != tab.opened.diff_id {
        refresh::apply(tab, old.diff_id, Arc::new(provider), plan, cx);
    }
    crate::view_state::save_now(tab, cx);
    reload(tab, cx);
    cx.notify();
}

/// The banner strip's line when no banner shows: what the tab compares.
fn update_context(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let text = context_line(tab);
    tab.banners.update(cx, |b, cx| {
        if *b.context() != text {
            b.set_context(text, cx);
        }
    });
}

/// The banner strip's line for what the tab shows.
pub fn context_line(tab: &ReviewTab) -> SharedString {
    let Some(s) = state(tab) else {
        return crate::review_tab::description(&tab.opened).into();
    };
    let opened = &tab.opened;
    let files = match opened.files.len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let head = head_name(opened.head_commit.as_ref(), &opened.head_tree);
    match s.showing {
        Showing::Current => crate::review_tab::description(opened).into(),
        Showing::Iteration(k) => {
            let base = base_name(opened);
            format!(
                "Iteration {k} of {} · {base} → {head} · {files} changed",
                s.count()
            )
            .into()
        }
        Showing::ChangesSince { since } => {
            let old = head_name(opened.base.commit.as_ref(), &opened.base.tree);
            let now = match s.current_seq() {
                Some(k) => format!("iteration {k} ({head})"),
                None => head,
            };
            format!(
                "Changes since your last review · iteration {since} ({old}) → {now} · \
                 {files} changed · comments on new lines only"
            )
            .into()
        }
    }
}

/// The base as the banner line names it: its ref and commit, else the
/// commit, else the tree.
fn base_name(opened: &OpenedDiff) -> String {
    match (&opened.base.ref_name, &opened.base.commit) {
        (Some(r), Some(c)) => format!("{} ({})", short_ref(r), c.short()),
        (None, Some(c)) => c.short().to_string(),
        (Some(r), None) => short_ref(r).to_owned(),
        (None, None) => format!("tree {}", opened.base.tree.short()),
    }
}
