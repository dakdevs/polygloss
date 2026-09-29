//! The store feed in the app (design §7.1 "Change notification", §11.7,
//! §17, T3.13): what other processes (`polygloss mcp`, the CLI, another
//! app) write to the store reaches the open tabs and Home.
//!
//! - [`StoreFeed`] (a GPUI global) owns the store's [`EventFeed`], a
//!   dedicated read connection, and polls it on the background executor:
//!   every [`FOCUSED_INTERVAL`] while the app is active, every
//!   [`UNFOCUSED_INTERVAL`] otherwise, and at once on [`nudge`] (the
//!   socket's `store_changed`, M4) or when the main window becomes active.
//!   A poll costs one `PRAGMA data_version` unless another connection
//!   committed. The feed starts at the latest event: tabs and Home read
//!   their state from the store when they open, events only tell them to
//!   read again.
//! - New events go to [`on_events`] hooks, then to Home (a reload) and to
//!   every review tab ([`tab::on_events`]): thread and comment events of the
//!   tab's review reload its threads (only changed blocks are touched,
//!   T3.9), and agent activity, re-review requests and submissions re-read
//!   the review's [`ReviewActivity`](polygloss_core::review::ReviewActivity)
//!   for the "claude-code replied to N threads" and "Re-review requested"
//!   banners.

pub mod tab;

use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Global, MenuItem, Task, Window};
use polygloss_core::paths::DataPaths;
use polygloss_core::store::StoreError;
use polygloss_core::store::events::{Event, EventFeed, EventFilter, EventKind};

use crate::app_state::AppState;
use crate::keymap::actions::tab as tab_actions;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::tabs::TabItem;
use crate::window::{MainWindow, MenuKind};

pub use tab::{TabFeed, activity, next_unread, replies_text, rereview_text};

/// How often the feed polls while the app is active (design §7.1,
/// provisional).
pub const FOCUSED_INTERVAL: Duration = Duration::from_millis(150);

/// How often the feed polls while the app is in the background.
pub const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(1);

/// The poll interval for an app that is (or is not) active.
pub fn interval_for(focused: bool) -> Duration {
    if focused {
        FOCUSED_INTERVAL
    } else {
        UNFOCUSED_INTERVAL
    }
}

/// Counters for tests and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedStats {
    /// Polls finished (most read only `data_version`).
    pub polls: u64,
    /// Events handed out.
    pub events: u64,
    /// Polls that failed (the store could not be read).
    pub errors: u64,
}

/// The app's store feed (a GPUI global, set by [`init`]).
pub struct StoreFeed {
    nudges: mpsc::UnboundedSender<()>,
    stats: FeedStats,
    interval: Duration,
    /// The last poll failed (errors are logged once per failure streak).
    failing: bool,
    _task: Task<()>,
}

impl Global for StoreFeed {}

impl StoreFeed {
    pub fn global(cx: &App) -> &StoreFeed {
        cx.global::<StoreFeed>()
    }

    /// Polls at once instead of at the next tick (several nudges before the
    /// poll make one poll). Callable from any thread.
    pub fn nudge(&self) {
        // Fails only when the feed task is gone (the app is quitting).
        let _ = self.nudges.unbounded_send(());
    }

    pub fn stats(&self) -> FeedStats {
        self.stats
    }

    /// The wait before the next poll, as chosen after the last one.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

/// [`StoreFeed::nudge`] on the app's feed, if it runs.
pub fn nudge(cx: &App) {
    if let Some(feed) = cx.try_global::<StoreFeed>() {
        feed.nudge();
    }
}

type EventsHook = Rc<dyn Fn(&[Event], &mut App)>;

/// Hooks that get every batch of new events ([`on_events`]).
#[derive(Default)]
struct EventsHooks(Vec<EventsHook>);

impl Global for EventsHooks {}

/// Runs `hook` with every batch of new store events (in `seq` order),
/// before the tabs and Home get them (T3.17's notifications).
pub fn on_events(cx: &mut App, hook: impl Fn(&[Event], &mut App) + 'static) {
    cx.default_global::<EventsHooks>().0.push(Rc::new(hook));
}

/// Starts the feed, registers "Next unread reply" and polls at once when the
/// main window becomes active.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::NextUnreadThread, window, cx| {
            next_unread(tab, window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::Review,
        vec![
            MenuItem::separator(),
            MenuItem::action("Next Unread Reply", tab_actions::NextUnreadThread),
        ],
        cx,
    );
    cx.observe_new(
        |_: &mut MainWindow, window: Option<&mut Window>, cx: &mut Context<MainWindow>| {
            let Some(window) = window else {
                return;
            };
            cx.observe_window_activation(window, |_, window, cx| {
                if window.is_window_active() {
                    nudge(cx);
                }
            })
            .detach();
        },
    )
    .detach();
    if let Some(state) = cx.try_global::<AppState>() {
        let paths = state.paths.clone();
        start(paths, cx);
    }
}

/// Installs the [`StoreFeed`] global and its polling task.
fn start(paths: DataPaths, cx: &mut App) {
    let (nudges, rx) = mpsc::unbounded();
    let task = cx.spawn(async move |cx: &mut AsyncApp| run(paths, rx, cx).await);
    cx.set_global(StoreFeed {
        nudges,
        stats: FeedStats::default(),
        interval: FOCUSED_INTERVAL,
        failing: false,
        _task: task,
    });
}

/// The feed task: poll on the background executor, hand the events out on
/// the main thread, wait for the interval or a nudge, repeat.
async fn run(paths: DataPaths, mut nudges: mpsc::UnboundedReceiver<()>, cx: &mut AsyncApp) {
    let mut feed: Option<EventFeed> = None;
    loop {
        let taken = feed.take();
        let at = paths.clone();
        let (back, polled) = cx
            .background_spawn(async move { poll_once(taken, &at) })
            .await;
        feed = back;
        let interval = cx.update(|cx| after_poll(polled, cx));
        let timer = cx.background_executor().timer(interval);
        if let futures::future::Either::Right((None, _)) =
            futures::future::select(timer, nudges.next()).await
        {
            // The global and its sender are gone.
            break;
        }
        // Nudges that came in meanwhile are served by this poll.
        while nudges.try_recv().is_ok() {}
    }
}

/// One poll, opening the feed at the latest event first when needed. The
/// feed comes back for the next poll (`None` when it could not be opened).
fn poll_once(
    feed: Option<EventFeed>,
    paths: &DataPaths,
) -> (Option<EventFeed>, Result<Vec<Event>, StoreError>) {
    let mut feed = match feed {
        Some(feed) => feed,
        None => {
            let opened = EventFeed::open(paths, 0, EventFilter::default()).and_then(|mut f| {
                f.seek_to_latest()?;
                Ok(f)
            });
            match opened {
                Ok(feed) => feed,
                Err(e) => return (None, Err(e)),
            }
        }
    };
    let events = feed.poll();
    (Some(feed), events)
}

/// Records a poll, hands its events out and returns the wait before the
/// next one.
fn after_poll(polled: Result<Vec<Event>, StoreError>, cx: &mut App) -> Duration {
    let interval = interval_for(cx.active_window().is_some());
    let events = if cx.has_global::<StoreFeed>() {
        let feed = cx.global_mut::<StoreFeed>();
        feed.stats.polls += 1;
        feed.interval = interval;
        match polled {
            Ok(events) => {
                feed.failing = false;
                feed.stats.events += events.len() as u64;
                events
            }
            Err(e) => {
                feed.stats.errors += 1;
                if !feed.failing {
                    tracing::warn!("reading the store's events: {e}");
                }
                feed.failing = true;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if !events.is_empty() {
        dispatch(&events, cx);
    }
    interval
}

/// Hands `events` to the hooks, Home and every review tab.
fn dispatch(events: &[Event], cx: &mut App) {
    let hooks = cx
        .try_global::<EventsHooks>()
        .map(|h| h.0.clone())
        .unwrap_or_default();
    for hook in hooks {
        hook(events, cx);
    }
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    let items: Vec<TabItem> = main.read(cx).tabs().items().to_vec();
    // Drafts only show in their review's tab (and are written by this app).
    let home = events.iter().any(|e| e.kind != EventKind::DraftChanged);
    for item in items {
        match item {
            TabItem::Home(view) => {
                if home {
                    view.update(cx, |h, cx| h.refresh(cx));
                }
            }
            TabItem::Review(review) => {
                review.update(cx, |t, cx| tab::on_events(t, events, cx));
            }
        }
    }
}

/// Sets the store feed up on a new review tab: reads the review's agent
/// activity for its banners.
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    tab::attach(tab, window, cx);
}
