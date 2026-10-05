//! Home: recent reviews across repos (design §11.2), the main window's first
//! tab. It renders the window's shell ([`crate::chrome::shell`]): the
//! sidebar on its Reviews segment ([`nav`]; Files is disabled here) and the
//! main column, a toolbar row ("Reviews", the count) over the list.
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

pub mod dialogs;
pub mod nav;
pub mod prune;
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
use crate::home::row::{HomeRow, RowPlace};

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
        let (awaiting, recent): (Vec<_>, Vec<_>) =
            loaded.summaries.into_iter().partition(|s| s.awaiting_you);
        self.awaiting = awaiting.len();
        self.rows = awaiting.into_iter().chain(recent).map(make).collect();
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

    /// Archives the review (hidden from Home until it is opened again).
    pub fn archive(&mut self, review_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.run("archive the review", window, cx, move |core| {
            core.archive_review(&review_id, &Actor::human())
        });
    }

    /// Asks for confirmation, then prunes the review ([`HomeView::prune`]).
    pub fn request_prune(&mut self, review_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row(review_id) else {
            return;
        };
        dialogs::confirm_prune(
            cx.entity().downgrade(),
            review_id.to_owned(),
            row.title.clone(),
            row.summary.repo_display.clone().into(),
            window,
            cx,
        );
    }

    /// Deletes the review and everything hanging off it (OQ-24), without
    /// asking; the UI asks first ([`HomeView::request_prune`]).
    pub fn prune(&mut self, review_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.run("prune the review", window, cx, move |core| {
            core.prune_review(&review_id)
        });
    }

    /// Mutes the review's notifications, or unmutes them.
    pub fn toggle_mute(&mut self, review_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.row(review_id) else {
            return;
        };
        let (id, muted) = (review_id.to_owned(), !row.summary.muted);
        self.run("change mute", window, cx, move |core| {
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
        self.run("assign the review", window, cx, move |core| {
            core.assign_review(&review_id, &session_id, AssignedBy::Human)
        });
    }

    /// Runs `f` on the background executor, then reloads; a failure is a
    /// toast ("Could not <what>: …").
    fn run(
        &mut self,
        what: &'static str,
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
        let item =
            |label: &'static str,
             f: fn(&mut HomeView, String, &mut Window, &mut Context<HomeView>)| {
                let (home, id) = (home.clone(), review_id.clone());
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    home.update(cx, |h, cx| f(h, id.clone(), window, cx)).ok();
                })
            };
        menu.item(item("Open", |h, id, w, cx| h.open_row(&id, w, cx)))
            .item(item(
                if muted { "Unmute" } else { "Mute" },
                |h, id, w, cx| h.toggle_mute(&id, w, cx),
            ))
            .item(item("Assign to session…", |h, id, w, cx| {
                h.request_assign(&id, w, cx)
            }))
            .separator()
            .item(item("Archive", |h, id, w, cx| h.archive(id, w, cx)))
            .item(item("Prune…", |h, id, w, cx| h.request_prune(&id, w, cx)))
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

    fn render_rows(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let _ = window;
        let now = (self.clock)();
        let offset = row::local_utc_offset_s(now);
        let theme = cx.theme().clone();
        let section = |title: &'static str, count: usize, first: bool| {
            h_flex()
                .gap_2()
                .when(!first, |d| d.pt_4())
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(title)
                .child(div().font_normal().child(count.to_string()))
                .into_any_element()
        };
        let mut children = Vec::with_capacity(self.rows.len() + 2);
        let home = cx.entity().downgrade();
        let selected = self.selected_index();
        for (ix, row) in self.rows.iter().enumerate() {
            let in_awaiting = ix < self.awaiting;
            if ix == 0 && in_awaiting {
                children.push(section("AWAITING YOU", self.awaiting, true));
            }
            if ix == self.awaiting {
                children.push(section(
                    "RECENT",
                    self.rows.len() - self.awaiting,
                    self.awaiting == 0,
                ));
            }
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
            children.push(
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
            );
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
        let sidebar = v_flex()
            .size_full()
            .child(crate::chrome::sidebar_top_row(false, window, cx))
            .child(div().flex_1().min_h_0().child(nav::render_nav(window, cx)))
            .into_any_element();
        let theme = cx.theme().clone();
        let count = (self.loaded && !self.rows.is_empty()).then(|| {
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(match self.rows.len() {
                    1 => "1 review".to_owned(),
                    n => format!("{n} reviews"),
                }))
                .into_any_element()
        });
        let title = div()
            .text_base()
            .font_semibold()
            .text_color(theme.foreground)
            .child("Reviews")
            .into_any_element();
        let toolbar = crate::chrome::toolbar_row(
            "home-toolbar",
            std::iter::once(title).chain(count).collect(),
            Vec::new(),
            window,
            cx,
        );
        let page = if self.loaded && self.rows.is_empty() {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(theme.foreground)
                        .child("No reviews yet"),
                )
                .child(
                    div()
                        .text_sm()
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
                .px_8()
                .pt_6()
                .pb_10()
                .gap_2()
                // One child per section title and row, so `scroll_to_item`
                // finds rows by index.
                .children(
                    self.render_rows(window, cx)
                        .into_iter()
                        .map(|child| div().flex_none().w_full().max_w(px(1120.)).child(child)),
                )
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
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|h, _: &SelectNext, _, cx| h.move_selection(1, cx)))
            .on_action(cx.listener(|h, _: &SelectPrev, _, cx| h.move_selection(-1, cx)))
            .on_action(cx.listener(|h, _: &OpenSelected, window, cx| {
                h.with_selected(window, cx, |h, id, w, cx| h.open_row(&id, w, cx))
            }))
            .on_action(cx.listener(|h, _: &Archive, window, cx| {
                h.with_selected(window, cx, |h, id, w, cx| h.archive(id, w, cx))
            }))
            .on_action(cx.listener(|h, _: &Prune, window, cx| {
                h.with_selected(window, cx, |h, id, w, cx| h.request_prune(&id, w, cx))
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
