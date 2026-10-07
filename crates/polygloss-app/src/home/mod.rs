//! Home: recent reviews across repos (design §11.2), the main window's first
//! tab. It renders the window's shell ([`crate::chrome::shell`]): the
//! sidebar on its Reviews segment ([`nav`]; Files is disabled here) and the
//! main column, a toolbar row ("Reviews", the count, Open… for ⌘O's open
//! flow) over the list.
//!
//! [`HomeView`] lists `Core::review_summaries` in two sections, **Awaiting
//! you** (re-review requested, or an open agent question without a human
//! reply) and **Recent**, each most recently active first. A row
//! ([`row`]) shows the repo, title, kind, status or last verdict, viewed
//! N/M, open threads, the assigned agent and the time of the last activity.
//! Row actions (the ⋯ menu, a right click, or the keyboard on the selected
//! row): open (click, `enter`), archive (`e`), prune (`⌘⌫`, confirmed),
//! mute (`m`) and "Assign to session…" (`a`, [`AssignToSession`], OQ-32).
//! `j`/`k` or the arrows select a row.
//!
//! Home reloads on focus, when the window becomes active, after its own
//! actions and after an automatic prune ([`prune`]); T3.13's store feed
//! calls [`HomeView::refresh`] on store events.
//!
//! A row archived or pruned leaves the model at once, and every reload
//! leaves it out until the store write is done (a reload read before the
//! write never brings it back); by pointer (the ⋯ and context menus, a prune
//! dialog of at most [`removal::ANIMATED_REMOVALS`] rows) it leaves the list
//! as M14 ([`removal`]), while Home is on screen; `e`, ⌘⌫, larger prunes,
//! automatic prunes, a removal that empties its section (whose title goes at
//! once) and one made from the sidebar on a review tab snap. Any other
//! change of the list, or Home leaving the screen, ends a removal under way.

pub mod dialogs;
pub mod nav;
pub mod prune;
pub mod removal;
pub mod row;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, MouseButton, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    TaskExt as _, WeakEntity, Window, div, px,
};
use polygloss_core::git::{Git, ReviewKind};
use polygloss_core::review::{AssignedBy, Core, CoreError, ReviewFilter, ReviewSummary};
use polygloss_core::store::events::{Actor, now_ms};

use crate::app_state::AppState;
use crate::home::removal::{Removals, Removed};
use crate::home::row::{HomeRow, RowPlace};
use crate::motion::settle::OnScreen;
use crate::motion::{self, Initiator};
use crate::space::{TextStyleExt as _, edge, gap, layout, text};

/// The key context of Home.
pub const CONTEXT: &str = "Home";

/// How far back "Assign to session…" lists sessions (OQ-32).
pub const ASSIGN_SESSIONS_WITHIN: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How often relative times are redrawn.
const TICK: Duration = Duration::from_secs(60);

gpui_kit::actions!(
    home,
    [
        /// `j` / `↓`: select the next row.
        SelectNext,
        /// `k` / `↑`: select the previous row.
        SelectPrev,
        /// `enter`: open the selected review (or focus its tab).
        OpenSelected,
        /// `e`: archive the selected review (reopening it un-archives it).
        Archive,
        /// `⌘⌫`: prune the selected review, after a confirmation.
        Prune,
        /// `m`: mute or unmute the selected review's notifications.
        ToggleMute,
        /// Reload the list.
        Refresh,
    ]
);

/// "Assign to session…" (OQ-32): reassign the selected review to another
/// agent session seen in the last 7 days. Declared once, in the action
/// registry (T3.2), since GPUI panics when two `actions!` register one name.
pub use crate::keymap::actions::tab::AssignToSession;

/// Home's key bindings and the automatic prune.
pub fn init(cx: &mut App) {
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("k", SelectPrev, ctx),
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("enter", OpenSelected, ctx),
        KeyBinding::new("e", Archive, ctx),
        KeyBinding::new("cmd-backspace", Prune, ctx),
        KeyBinding::new("m", ToggleMute, ctx),
        KeyBinding::new("a", AssignToSession, ctx),
        // T5.6: the list reloads by itself; `R` does it now, as in a review
        // tab.
        KeyBinding::new("shift-r", Refresh, ctx),
    ]);
    // "Assign to session…" from a review tab (the palette, T5.6): the
    // Home's picker, for the tab's review.
    crate::keymap::handlers::on_action(
        cx,
        |tab: &mut crate::review_tab::ReviewTab, _: &AssignToSession, window, cx| {
            let Some(home) = home_view(cx) else {
                return;
            };
            let (id, title) = (tab.review_id.clone(), tab.title());
            home.update(cx, |h, cx| h.pick_session_for(id, title, window, cx));
        },
    );
    prune::start(cx);
}

/// The main window's Home.
fn home_view(cx: &App) -> Option<Entity<HomeView>> {
    let (_, main) = crate::window::main_window(cx)?;
    main.read(cx).tabs().items().iter().find_map(|t| match t {
        crate::tabs::TabItem::Home(home) => Some(home.clone()),
        _ => None,
    })
}

/// What one reload read (on the background executor).
struct Loaded {
    summaries: Vec<ReviewSummary>,
    /// Canonical session id → client name.
    agents: HashMap<String, String>,
    /// Subjects looked up this time, by `(repo path, commit)`.
    subjects: Vec<((PathBuf, String), Option<String>)>,
}

/// The Home tab's view.
pub struct HomeView {
    focus: FocusHandle,
    /// Awaiting you first, then Recent; each most recently active first.
    rows: Vec<HomeRow>,
    /// How many of `rows` are awaiting you.
    awaiting: usize,
    /// Whether the first reload finished.
    loaded: bool,
    selected: Option<String>,
    /// Commit subjects by `(repo path, commit)`; `None` when git could not
    /// tell (the repo is gone).
    subjects: HashMap<(PathBuf, String), Option<String>>,
    refreshing: Option<Task<()>>,
    clock: Rc<dyn Fn() -> i64>,
    scroll: ScrollHandle,
    /// Rows leaving the list (M14).
    removals: Removals,
    /// Rows archived or pruned whose store write is not done: every reload
    /// leaves them out (one entry per removal, so overlapping removals of a
    /// row each keep it out).
    removing: Vec<String>,
    /// Whether Home was drawn on the window's latest frame: removals start
    /// and run only while it is.
    on_screen: OnScreen,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl HomeView {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<HomeView> {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            let subscriptions = vec![
                cx.on_focus(&focus, window, |home: &mut HomeView, _, cx| {
                    home.refresh(cx)
                }),
                cx.observe_window_activation(window, |home: &mut HomeView, window, cx| {
                    if window.is_window_active() {
                        home.refresh(cx);
                    }
                }),
                cx.observe_global::<prune::AutoPrune>(|home: &mut HomeView, cx| home.refresh(cx)),
            ];
            let tick = cx.spawn(async move |this: WeakEntity<HomeView>, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            let mut home = HomeView {
                focus,
                rows: Vec::new(),
                awaiting: 0,
                loaded: false,
                selected: None,
                subjects: HashMap::new(),
                refreshing: None,
                clock: Rc::new(now_ms),
                scroll: ScrollHandle::new(),
                removals: Removals::default(),
                removing: Vec::new(),
                on_screen: OnScreen::default(),
                _tick: tick,
                _subscriptions: subscriptions,
            };
            home.refresh(cx);
            home
        })
    }

    /// Every row: Awaiting you first, then Recent.
    pub fn rows(&self) -> &[HomeRow] {
        &self.rows
    }

    /// The Awaiting you section.
    pub fn awaiting_you(&self) -> &[HomeRow] {
        &self.rows[..self.awaiting]
    }

    /// The Recent section.
    pub fn recent(&self) -> &[HomeRow] {
        &self.rows[self.awaiting..]
    }

    /// Whether the first reload has finished.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// The selected review.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn select(&mut self, review_id: Option<&str>, cx: &mut Context<Self>) {
        self.selected = review_id.map(str::to_owned);
        self.scroll_to_selected();
        cx.notify();
    }

    /// Uses `clock` (Unix ms) for relative times (screenshots pin it).
    pub fn set_clock(&mut self, clock: impl Fn() -> i64 + 'static, cx: &mut Context<Self>) {
        self.clock = Rc::new(clock);
        cx.notify();
    }

    fn row(&self, review_id: &str) -> Option<&HomeRow> {
        self.rows.iter().find(|r| r.review_id() == review_id)
    }

    /// Reloads the summaries on the background executor (a newer reload
    /// replaces one in flight).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let core = AppState::global(cx).core.clone();
        let known: Vec<(PathBuf, String)> = self.subjects.keys().cloned().collect();
        self.refreshing = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load(&core, &known) })
                .await;
            this.update(cx, |home, cx| match loaded {
                Ok(loaded) => home.apply(loaded, cx),
                Err(e) => tracing::warn!("loading Home: {e}"),
            })
            .ok();
        }));
    }

    /// Test seam: applies a reload that read `summaries` (one that was in
    /// flight while an action wrote the store, so it may predate the write).
    #[doc(hidden)]
    pub fn apply_summaries(&mut self, summaries: Vec<ReviewSummary>, cx: &mut Context<Self>) {
        let loaded = Loaded {
            summaries,
            agents: HashMap::new(),
            subjects: Vec::new(),
        };
        self.apply(loaded, cx);
    }

    fn apply(&mut self, loaded: Loaded, cx: &mut Context<Self>) {
        self.subjects.extend(loaded.subjects);
        let make = |s: ReviewSummary| {
            let subject = match (s.kind, row::parse_key(&s.key)) {
                (ReviewKind::Commit, Some(row::ParsedKey::Commit { oid })) => self
                    .subjects
                    .get(&(s.repo_path.clone(), oid))
                    .cloned()
                    .flatten(),
                _ => None,
            };
            let agent = s
                .assigned_session
                .as_ref()
                .map(|id| loaded.agents.get(id).cloned().unwrap_or_else(|| id.clone()));
            HomeRow::new(s, subject.as_deref(), agent.as_deref())
        };
        // A row being removed stays out, whenever the reload read the store.
        let removing = &self.removing;
        let (awaiting, recent): (Vec<_>, Vec<_>) = loaded
            .summaries
            .into_iter()
            .filter(|s| !removing.contains(&s.review_id))
            .partition(|s| s.awaiting_you);
        let count = awaiting.len();
        let rows: Vec<HomeRow> = awaiting.into_iter().chain(recent).map(make).collect();
        // Any change but the one a removal already made ends it (M14).
        if rows != self.rows || count != self.awaiting {
            self.removals.clear();
        }
        self.awaiting = count;
        self.rows = rows;
        if let Some(sel) = &self.selected
            && self.row(sel).is_none()
        {
            self.selected = None;
        }
        self.loaded = true;
        self.refreshing = None;
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_deref()?;
        self.rows.iter().position(|r| r.review_id() == sel)
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let ix = match self.selected_index() {
            None if delta > 0 => 0,
            None => last,
            Some(ix) => ix.saturating_add_signed(delta).min(last),
        };
        let id = self.rows[ix].review_id().to_owned();
        self.select(Some(&id), cx);
    }

    /// Scrolls the selected row into view. The list's children are the
    /// section titles and the rows (see `render`).
    fn scroll_to_selected(&self) {
        let Some(ix) = self.selected_index() else {
            return;
        };
        // "Awaiting you" title (when shown), rows, "Recent" title.
        let mut child = ix;
        if self.awaiting > 0 {
            child += 1;
        }
        if ix >= self.awaiting {
            child += 1;
        }
        self.scroll.scroll_to_item(child);
    }

    /// Opens the review of row `review_id`: focuses its tab when it is open,
    /// else opens it (`Core::open` on the background executor; errors show
    /// as toasts).
    pub fn open_row(&mut self, review_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row(review_id) else {
            return;
        };
        if let Some((_, main)) = crate::window::main_window(cx) {
            let ix = main.read(cx).tabs().find_review(review_id, cx);
            if let Some(ix) = ix {
                main.update(cx, |main, cx| main.activate_tab(ix, window, cx));
                return;
            }
        }
        let Some(req) = row::open_request(&row.summary) else {
            let key = row.summary.key.clone();
            toast(
                format!("Cannot open this review: unknown key {key:?}"),
                window,
                cx,
            );
            return;
        };
        crate::review_tab::open_review(req, window, cx).detach_and_log_err(cx);
    }

    /// Archives the review (hidden from Home until it is opened again). Its
    /// row leaves at once, as `initiator` plays it (M14).
    pub fn archive(
        &mut self,
        review_id: String,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = vec![review_id.clone()];
        self.remove_rows(&ids, initiator, window, cx);
        self.run("archive the review", ids, window, cx, move |core| {
            core.archive_review(&review_id, &Actor::human())
        });
    }

    /// Asks for confirmation, then prunes the reviews
    /// ([`HomeView::prune`]); `initiator` asked (⌘⌫, a menu), and the
    /// confirmation's own initiator counts too.
    pub fn request_prune(
        &mut self,
        review_ids: Vec<String>,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows: Vec<dialogs::PruneRow> = review_ids
            .iter()
            .filter_map(|id| self.row(id))
            .map(|row| dialogs::PruneRow {
                review_id: row.review_id().to_owned(),
                title: row.title.clone(),
                repo: row.summary.repo_display.clone().into(),
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        dialogs::confirm_prune(cx.entity().downgrade(), rows, initiator, window, cx);
    }

    /// Deletes the reviews and everything hanging off them (OQ-24), without
    /// asking; the UI asks first ([`HomeView::request_prune`]). Their rows
    /// leave at once, as `initiator` plays it (M14).
    pub fn prune(
        &mut self,
        review_ids: Vec<String>,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remove_rows(&review_ids, initiator, window, cx);
        let removing = review_ids.clone();
        self.run("prune the reviews", removing, window, cx, move |core| {
            review_ids.iter().try_for_each(|id| core.prune_review(id))
        });
    }

    /// Drops the rows of `review_ids` from the list now (commit first) and
    /// keeps them out of every reload until the store write is done (its
    /// reload confirms it, or brings a row back with the error); while Home
    /// is on screen they leave as `initiator` plays it ([`Removals::start`]).
    /// Removals under way end first.
    fn remove_rows(
        &mut self,
        review_ids: &[String],
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.removals.clear();
        self.removing.extend(review_ids.iter().cloned());
        let gone = |row: &HomeRow| review_ids.iter().any(|id| id == row.review_id());
        if !self.rows.iter().any(gone) {
            return;
        }
        let (before_awaiting, before_recent) = (self.awaiting, self.rows.len() - self.awaiting);
        let mut kept = Vec::with_capacity(self.rows.len());
        let mut removed = Vec::new();
        let mut awaiting = 0;
        for (ix, row) in std::mem::take(&mut self.rows).into_iter().enumerate() {
            let in_awaiting = ix < before_awaiting;
            if gone(&row) {
                removed.push(Removed {
                    before: kept.len(),
                    awaiting: in_awaiting,
                    row,
                });
            } else {
                awaiting += usize::from(in_awaiting);
                kept.push(row);
            }
        }
        let recent = kept.len() - awaiting;
        let emptied = (before_awaiting > 0 && awaiting == 0) || (before_recent > 0 && recent == 0);
        self.rows = kept;
        self.awaiting = awaiting;
        if self
            .selected
            .as_deref()
            .is_some_and(|sel| self.row(sel).is_none())
        {
            self.selected = None;
        }
        // From the sidebar on a review tab: nothing on screen leaves.
        if self.on_screen.was_drawn(window, cx) {
            let now_ms = (self.clock)();
            self.removals
                .start(removed, emptied, now_ms, initiator, window, cx);
        }
        cx.notify();
    }

    /// Mutes the review's notifications, or unmutes them.
    pub fn toggle_mute(&mut self, review_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row(review_id) else {
            return;
        };
        let (id, muted) = (review_id.to_owned(), !row.summary.muted);
        self.run("change mute", Vec::new(), window, cx, move |core| {
            core.set_muted(&id, muted)
        });
    }

    /// Lists the sessions seen in the last 7 days and lets the user pick
    /// the one the review is assigned to (OQ-32).
    pub fn request_assign(&mut self, review_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row(review_id) else {
            return;
        };
        let title = row.title.clone();
        self.pick_session_for(review_id.to_owned(), title, window, cx);
    }

    /// The session picker for review `review_id`, titled `title` (a review
    /// tab's `tab::AssignToSession` too, T5.6): the sessions and the
    /// current assignee are read off the main thread.
    pub fn pick_session_for(
        &mut self,
        review_id: String,
        title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let core = AppState::global(cx).core.clone();
        let since = (self.clock)() - ASSIGN_SESSIONS_WITHIN.as_millis() as i64;
        let id = review_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_spawn(async move {
                    let current = core.assigned_session(&id)?.map(|s| s.id);
                    Ok::<_, CoreError>((core.recent_sessions(since)?, current))
                })
                .await;
            this.update_in(cx, |_, window, cx| match read {
                Ok((sessions, current)) => dialogs::pick_session(
                    cx.entity().downgrade(),
                    review_id,
                    title,
                    current,
                    sessions,
                    window,
                    cx,
                ),
                Err(e) => toast(format!("Could not list sessions: {e}"), window, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Assigns the review to `session_id` as the human.
    pub fn assign(
        &mut self,
        review_id: String,
        session_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run("assign the review", Vec::new(), window, cx, move |core| {
            core.assign_review(&review_id, &session_id, AssignedBy::Human)
        });
    }

    /// Runs `f` on the background executor, then lets reloads show the rows
    /// of `removed` again (one entry each of [`HomeView::remove_rows`]'s)
    /// and reloads; a failure is a toast ("Could not <what>: …").
    fn run(
        &mut self,
        what: &'static str,
        removed: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&Core) -> Result<(), CoreError> + Send + 'static,
    ) {
        let core = AppState::global(cx).core.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { f(&core) }).await;
            this.update_in(cx, |home, window, cx| {
                if let Err(e) = result {
                    toast(format!("Could not {what}: {e}"), window, cx);
                }
                for id in &removed {
                    if let Some(ix) = home.removing.iter().position(|r| r == id) {
                        home.removing.swap_remove(ix);
                    }
                }
                home.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn with_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, String, &mut Window, &mut Context<Self>),
    ) {
        if let Some(id) = self.selected.clone() {
            f(self, id, window, cx);
        }
    }

    /// The ⋯ menu and the context menu of row `row` (Home's and the
    /// sidebar's Reviews segment's): open, mute or unmute, assign to a
    /// session, archive, prune. Its items act on the row's review by id.
    pub(crate) fn row_menu(
        &self,
        row: usize,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some(r) = self.rows.get(row) else {
            return menu;
        };
        let (home, review_id, muted) = (
            cx.entity().downgrade(),
            r.review_id().to_owned(),
            r.summary.muted,
        );
        // Each item reads who chose it (a click, or the menu's keys).
        let item = |label: &'static str,
                    f: fn(
            &mut HomeView,
            String,
            Initiator,
            &mut Window,
            &mut Context<HomeView>,
        )| {
            let (home, id) = (home.clone(), review_id.clone());
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                let initiator = motion::initiator(window);
                home.update(cx, |h, cx| f(h, id.clone(), initiator, window, cx))
                    .ok();
            })
        };
        menu.item(item("Open", |h, id, _, w, cx| h.open_row(&id, w, cx)))
            .item(item(
                if muted { "Unmute" } else { "Mute" },
                |h, id, _, w, cx| h.toggle_mute(&id, w, cx),
            ))
            .item(item("Assign to session…", |h, id, _, w, cx| {
                h.request_assign(&id, w, cx)
            }))
            .separator()
            .item(item("Archive", |h, id, i, w, cx| h.archive(id, i, w, cx)))
            .item(item("Prune…", |h, id, i, w, cx| {
                h.request_prune(vec![id], i, w, cx)
            }))
    }

    /// [`HomeView::row_menu`] of `review_id`'s row as the menu opens (`menu`
    /// unchanged when Home or the row is gone by then).
    pub(crate) fn menu_for(
        home: &WeakEntity<HomeView>,
        review_id: &str,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut App,
    ) -> PopupMenu {
        let Some(home) = home.upgrade() else {
            return menu;
        };
        home.update(cx, |h, cx| {
            match h.rows.iter().position(|r| r.review_id() == review_id) {
                Some(row) => h.row_menu(row, menu, window, cx),
                None => menu,
            }
        })
    }

    /// The list's children: section titles and rows (each in the rows'
    /// width), and in a removed row's place its leaving slot (M14), a direct
    /// child of the list in the rows' width styles.
    fn render_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let now = (self.clock)();
        let offset = row::local_utc_offset_s(now);
        let theme = cx.theme().clone();
        let wrap = |child: AnyElement| {
            div()
                .flex_none()
                .w_full()
                .max_w(px(layout::HOME_MAX_W))
                .child(child)
                .into_any_element()
        };
        // A later section's title sits `SECTION` below the card before it:
        // the list's `CARDS` gap and the rest as padding.
        let section = |title: &'static str, count: usize, first: bool| {
            wrap(
                h_flex()
                    .gap(px(gap::CONTROLS))
                    .when(!first, |d| d.pt(px(gap::SECTION - gap::CARDS)))
                    .text_style(text::SMALL)
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .debug_selector(move || format!("home-section-{title}"))
                            .child(title),
                    )
                    .child(div().font_normal().child(count.to_string()))
                    .into_any_element(),
            )
        };
        // Home was off screen (a review tab showed): what left meanwhile is
        // gone, never replayed.
        if !self.on_screen.was_drawn(window, cx) {
            self.removals.clear();
        }
        let mut slots = self.removals.slots(window, cx).into_iter().peekable();
        let mut children = Vec::with_capacity(self.rows.len() + 2);
        let home = cx.entity().downgrade();
        let selected = self.selected_index();
        for ix in 0..=self.rows.len() {
            if ix == 0 && self.awaiting > 0 {
                children.push(section("AWAITING YOU", self.awaiting, true));
            }
            while slots.peek().is_some_and(|s| s.before == ix && s.awaiting) {
                children.extend(slots.next().map(|s| s.element.into_any_element()));
            }
            if ix == self.awaiting && self.rows.len() > self.awaiting {
                children.push(section(
                    "RECENT",
                    self.rows.len() - self.awaiting,
                    self.awaiting == 0,
                ));
            }
            while slots.peek().is_some_and(|s| s.before == ix) {
                children.extend(slots.next().map(|s| s.element.into_any_element()));
            }
            let Some(row) = self.rows.get(ix) else {
                break;
            };
            let id = row.review_id().to_owned();
            let menu_button = {
                let (home, id) = (home.clone(), id.clone());
                div()
                    // The row opens on click; the ⋯ button must not.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new(("home-row-menu", ix))
                            .icon(IconName::Ellipsis)
                            .ghost()
                            .small()
                            .tooltip("Actions")
                            .dropdown_menu_with_anchor(
                                Anchor::TopRight,
                                move |menu, window, cx| {
                                    HomeView::menu_for(&home, &id, menu, window, cx)
                                },
                            ),
                    )
                    .into_any_element()
            };
            let place = RowPlace {
                ix,
                selected: selected == Some(ix),
            };
            let (open_home, open_id) = (home.clone(), id.clone());
            let (ctx_home, ctx_id) = (home.clone(), id.clone());
            children.push(wrap(
                row::render_row(row, place, now, offset, menu_button, cx)
                    .on_click(move |_, window, cx| {
                        open_home
                            .update(cx, |h, cx| {
                                h.select(Some(&open_id), cx);
                                h.open_row(&open_id, window, cx)
                            })
                            .ok();
                    })
                    .context_menu(move |menu, window, cx| {
                        HomeView::menu_for(&ctx_home, &ctx_id, menu, window, cx)
                    })
                    .into_any_element(),
            ));
        }
        children
    }
}

/// Reads the summaries, the agents' names and the subjects of commits not
/// in `known`.
fn load(core: &Core, known: &[(PathBuf, String)]) -> Result<Loaded, CoreError> {
    let summaries = core.review_summaries(&ReviewFilter::default())?;
    let agents = core
        .recent_sessions(0)?
        .into_iter()
        .map(|s| (s.id, s.client_name))
        .collect();
    let mut subjects = Vec::new();
    for s in &summaries {
        if let Some(row::ParsedKey::Commit { oid }) = row::parse_key(&s.key) {
            let key = (s.repo_path.clone(), oid);
            if !known.contains(&key) && !subjects.iter().any(|(k, _)| *k == key) {
                let subject = commit_subject(&key.0, &key.1);
                subjects.push((key, subject));
            }
        }
    }
    Ok(Loaded {
        summaries,
        agents,
        subjects,
    })
}

/// The subject line of `commit` in the repo at `repo`, if git can read it.
pub(crate) fn commit_subject(repo: &std::path::Path, commit: &str) -> Option<String> {
    if !repo.is_dir() || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let out = Git::new(repo)
        .output(&[
            OsStr::new("log"),
            OsStr::new("-1"),
            OsStr::new("--no-walk"),
            OsStr::new("--format=%s"),
            OsStr::new(commit),
            OsStr::new("--"),
        ])
        .ok()?;
    let subject = String::from_utf8_lossy(&out).trim().to_owned();
    (!subject.is_empty()).then_some(subject)
}

/// Shows `message` as an error toast in the main window.
fn toast(message: String, window: &mut Window, cx: &mut App) {
    tracing::warn!("{message}");
    if let Some((_, main)) = crate::window::main_window(cx) {
        main.update(cx, |main, cx| main.toast_error(message.into(), window, cx));
    }
}

impl Focusable for HomeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = |window: &mut Window, cx: &mut App| {
            v_flex()
                .size_full()
                .child(crate::chrome::sidebar_top_row(false, window, cx))
                .child(div().flex_1().min_h_0().child(nav::render_nav(window, cx)))
                .into_any_element()
        };
        let theme = cx.theme().clone();
        let count = (self.loaded && !self.rows.is_empty()).then(|| {
            div()
                .text_style(text::SMALL)
                .text_color(theme.muted_foreground)
                .child(SharedString::from(match self.rows.len() {
                    1 => "1 review".to_owned(),
                    n => format!("{n} reviews"),
                }))
                .into_any_element()
        });
        let title = div()
            .text_style(text::HEADING)
            .font_semibold()
            .text_color(theme.foreground)
            .child("Reviews")
            .into_any_element();
        let open = Button::new("home-open")
            .debug_selector(|| "home-open".into())
            .icon(IconName::Plus)
            .label("Open…")
            .outline()
            .small()
            .tooltip("Open a review (⌘O)")
            .on_click(|_, window, cx| {
                crate::open_flow::open(window, cx);
            })
            .into_any_element();
        let toolbar = crate::chrome::toolbar_row(
            "home-toolbar",
            std::iter::once(title).chain(count).collect(),
            vec![open],
            false,
            window,
            cx,
        );
        let page = if self.loaded && self.rows.is_empty() {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_style(text::HEADING)
                        .text_color(theme.foreground)
                        .child("No reviews yet"),
                )
                .child(
                    div()
                        .text_style(text::BODY)
                        .text_color(theme.muted_foreground)
                        .child("Run `polygloss` in a repository, or press ⌘O to open one."),
                )
                .into_any_element()
        } else {
            v_flex()
                .id("home-scroll")
                .debug_selector(|| "home-list".into())
                .size_full()
                .items_center()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .px(px(edge::PAGE))
                .pt(px(gap::SECTION))
                .pb(px(edge::PAGE))
                .gap(px(gap::CARDS))
                // One child per section title and row (and leaving slot), so
                // `scroll_to_item` finds rows by index.
                .children(self.render_rows(window, cx))
                .into_any_element()
        };
        // The rows are cards on the diff's canvas (design §11.2).
        let canvas = crate::theme::viewport_theme(cx).canvas;
        let main = v_flex()
            .size_full()
            .bg(canvas)
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(page))
            .into_any_element();
        let shell = crate::chrome::shell(sidebar, main, false, window, cx);
        self.removals.register(window, cx);
        self.on_screen.mark(window, cx);
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|h, _: &SelectNext, _, cx| h.move_selection(1, cx)))
            .on_action(cx.listener(|h, _: &SelectPrev, _, cx| h.move_selection(-1, cx)))
            .on_action(cx.listener(|h, _: &OpenSelected, window, cx| {
                h.with_selected(window, cx, |h, id, w, cx| h.open_row(&id, w, cx))
            }))
            .on_action(cx.listener(|h, _: &Archive, window, cx| {
                let initiator = motion::initiator(window);
                h.with_selected(window, cx, |h, id, w, cx| h.archive(id, initiator, w, cx))
            }))
            .on_action(cx.listener(|h, _: &Prune, window, cx| {
                let initiator = motion::initiator(window);
                h.with_selected(window, cx, |h, id, w, cx| {
                    h.request_prune(vec![id], initiator, w, cx)
                })
            }))
            .on_action(cx.listener(|h, _: &ToggleMute, window, cx| {
                h.with_selected(window, cx, |h, id, w, cx| h.toggle_mute(&id, w, cx))
            }))
            .on_action(cx.listener(|h, _: &AssignToSession, window, cx| {
                h.with_selected(window, cx, |h, id, w, cx| h.request_assign(&id, w, cx))
            }))
            .on_action(cx.listener(|h, _: &Refresh, _, cx| h.refresh(cx)))
            .size_full()
            .child(shell)
    }
}
