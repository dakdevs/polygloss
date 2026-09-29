//! The open flow (⌘O, design §11.3): a dialog in two steps.
//!
//! 1. **Repo** ([`repo_step`]): the recent repos (`Core::recent_repos`),
//!    ranked with nucleo's path matching as you type, and "Browse…", a native
//!    folder picker (`cx.prompt_for_paths`). Any folder inside a worktree
//!    opens that worktree's repo.
//! 2. **Source** ([`source_step`]): Working tree (live, with its base),
//!    Commit (HEAD's log, virtualized and fuzzy) or Compare (base and head
//!    pickers, three-dot by default, a direct toggle and a label).
//!
//! ⌘⏎ (or Enter in the commit list or the label field) closes the dialog and
//! opens the review with `review_tab::open_review`, which focuses the tab of
//! a review that is already open. Keys inside the dialog (context
//! [`CONTEXT`]): ⌘1/⌘2/⌘3 pick the source kind, ⌘[ goes back to the repos,
//! Esc closes it. The repo and the refs are read on the background executor.

pub mod ranking;
pub mod repo_step;
pub mod source_step;

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::list::{List, ListEvent, ListState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    InteractiveElement as _, IntoElement, KeyBinding, Keystroke, MenuItem, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task, TaskExt as _,
    WeakEntity, Window, div, px,
};

use crate::app_state::AppState;
use crate::keymap::actions::window as window_actions;
use crate::keymap::handlers;
use crate::open_flow::repo_step::{RECENT_REPOS, RepoChoice, RepoDelegate, load_rows, tildify};
use crate::open_flow::source_step::{RepoData, SourceEvent, SourceMode, SourceStep, load_repo};
use crate::window::{MainWindow, MenuKind};

/// The dialog's key context.
pub const CONTEXT: &str = "OpenFlow";

/// The dialog's size, in points.
const WIDTH: f32 = 640.;
const HEIGHT: f32 = 460.;

gpui_kit::actions!(
    open_flow,
    [
        /// ⌘[: back to the repo list.
        Back,
        /// ⌘⏎: open what the source step names.
        Open,
        /// ⌘1: review the working tree.
        ModeLive,
        /// ⌘2: review a commit.
        ModeCommit,
        /// ⌘3: compare two refs.
        ModeCompare,
    ]
);

/// ⌘O on the main window, the dialog's keys and File › Open Review….
pub fn init(cx: &mut App) {
    // Single-line text fields let ⌘[ (outdent) and ⌘⏎ through, so these
    // work from the search and label fields too.
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-[", Back, ctx),
        KeyBinding::new("cmd-enter", Open, ctx),
        KeyBinding::new("cmd-1", ModeLive, ctx),
        KeyBinding::new("cmd-2", ModeCommit, ctx),
        KeyBinding::new("cmd-3", ModeCompare, ctx),
    ]);
    handlers::on_action(
        cx,
        |_: &mut MainWindow, _: &window_actions::OpenFlow, window, cx| {
            open(window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::File,
        vec![
            MenuItem::action("Open Review…", window_actions::OpenFlow),
            MenuItem::separator(),
        ],
        cx,
    );
}

/// Which step the flow shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStep {
    Repo,
    Source,
}

/// The flow while it is open.
#[derive(Default)]
struct OpenFlowGlobal(Option<WeakEntity<OpenFlow>>);

impl Global for OpenFlowGlobal {}

/// The open flow's content (in a gpui-kit dialog).
pub struct OpenFlow {
    focus: FocusHandle,
    repos: Entity<ListState<RepoDelegate>>,
    source: Option<Entity<SourceStep>>,
    /// The repo being read (after a choice) and the read.
    choosing: Option<(PathBuf, Task<()>)>,
    error: Option<SharedString>,
    _loading_repos: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// Opens the flow over `window` (or refocuses it when it is open), its repo
/// search focused.
pub fn open(window: &mut Window, cx: &mut App) -> Entity<OpenFlow> {
    if let Some(flow) = current(cx)
        && window.has_active_dialog(cx)
    {
        flow.update(cx, |f, cx| f.focus_step(window, cx));
        return flow;
    }
    let flow = cx.new(|cx| OpenFlow::new(window, cx));
    cx.set_global(OpenFlowGlobal(Some(flow.downgrade())));
    let content = flow.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(WIDTH))
            .margin_top(px(72.))
            .close_button(false)
            .child(content.clone())
    });
    flow.update(cx, |f, cx| f.focus_step(window, cx));
    flow
}

/// The open flow, if it is open.
pub fn current(cx: &App) -> Option<Entity<OpenFlow>> {
    cx.try_global::<OpenFlowGlobal>()?.0.as_ref()?.upgrade()
}

impl OpenFlow {
    fn new(window: &mut Window, cx: &mut Context<OpenFlow>) -> OpenFlow {
        let repos =
            cx.new(|cx| ListState::new(RepoDelegate::default(), window, cx).searchable(true));
        let subscriptions = vec![cx.subscribe_in(
            &repos,
            window,
            |flow: &mut OpenFlow, repos, event: &ListEvent, window, cx| {
                if let ListEvent::Confirm(ix) = event {
                    match repos.read(cx).delegate().choice(*ix) {
                        Some(RepoChoice::Repo(path)) => flow.choose_repo(path, window, cx),
                        Some(RepoChoice::Browse) => flow.browse(window, cx),
                        None => {}
                    }
                }
            },
        )];
        let core = AppState::global(cx).core.clone();
        let loading = cx.spawn_in(window, async move |flow, cx| {
            let rows = cx
                .background_spawn(async move { core.recent_repos(RECENT_REPOS).map(load_rows) })
                .await;
            flow.update_in(cx, |flow, window, cx| match rows {
                Ok(rows) => flow.set_recent(rows, window, cx),
                Err(e) => {
                    tracing::warn!("reading the recent repos: {e}");
                    flow.set_recent(Vec::new(), window, cx);
                }
            })
            .ok();
        });
        OpenFlow {
            focus: cx.focus_handle(),
            repos,
            source: None,
            choosing: None,
            error: None,
            _loading_repos: loading,
            _subscriptions: subscriptions,
        }
    }

    fn set_recent(
        &mut self,
        rows: Vec<repo_step::RepoRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.repos.update(cx, |list, cx| {
            list.delegate_mut().set_rows(rows);
            let first = if list.delegate().matches().next().is_some() {
                IndexPath::new(0)
            } else {
                IndexPath::new(0).section(1)
            };
            list.set_selected_index(Some(first), window, cx);
            cx.notify();
        });
    }

    pub fn step(&self) -> FlowStep {
        if self.source.is_some() {
            FlowStep::Source
        } else {
            FlowStep::Repo
        }
    }

    /// The repo list.
    pub fn repos(&self) -> &Entity<ListState<RepoDelegate>> {
        &self.repos
    }

    /// The source step, once a repo is chosen.
    pub fn source(&self) -> Option<&Entity<SourceStep>> {
        self.source.as_ref()
    }

    /// Why the last choice did not open (not a repository, git failed).
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Reads the repo containing `path` on the background executor, then
    /// shows its source step (or the error).
    pub fn choose_repo(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let target = path.clone();
        let task = cx.spawn_in(window, async move |flow, cx| {
            let data = cx.background_spawn(async move { load_repo(&target) }).await;
            flow.update_in(cx, |flow, window, cx| {
                flow.choosing = None;
                match data {
                    Ok(data) => flow.show_source(data, window, cx),
                    Err(message) => {
                        flow.error = Some(message.into());
                        flow.focus_step(window, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.choosing = Some((path, task));
        cx.notify();
    }

    fn show_source(&mut self, data: RepoData, window: &mut Window, cx: &mut Context<Self>) {
        let source = cx.new(|cx| SourceStep::new(data, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &source,
            window,
            |flow: &mut OpenFlow, _, event: &SourceEvent, window, cx| match event {
                SourceEvent::Confirm => flow.confirm(window, cx),
            },
        ));
        // A changed choice clears the last "not ready" message.
        self._subscriptions
            .push(cx.observe(&source, |flow: &mut OpenFlow, _, cx| {
                if flow.error.take().is_some() {
                    cx.notify();
                }
            }));
        self.source = Some(source);
        self.focus_step(window, cx);
    }

    /// Asks for a folder with the native picker and chooses its repo.
    pub fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |flow, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            flow.update_in(cx, |flow, window, cx| flow.choose_repo(path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Back to the repo list.
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source.take().is_some() {
            self.error = None;
            self.focus_step(window, cx);
            cx.notify();
        }
    }

    /// Focuses the current step's first control.
    fn focus_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.source {
            None => self.repos.update(cx, |l, cx| l.focus(window, cx)),
            Some(source) => source.update(cx, |s, cx| s.focus_mode(window, cx)),
        }
    }

    fn set_mode(&mut self, mode: SourceMode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(source) = &self.source {
            source.update(cx, |s, cx| s.set_mode(mode, window, cx));
        }
    }

    /// Opens what the source step names: closes the dialog and opens (or
    /// focuses) the review's tab. Does nothing while the choices are
    /// incomplete.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = &self.source else {
            return;
        };
        let req = match source.read(cx).request(cx) {
            Ok(req) => req,
            Err(why) => {
                self.error = Some(why);
                cx.notify();
                return;
            }
        };
        window.close_dialog(cx);
        cx.set_global(OpenFlowGlobal(None));
        crate::review_tab::open_review(req, window, cx).detach_and_log_err(cx);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (title, subtitle): (SharedString, SharedString) = match &self.source {
            None => (
                "Open a review".into(),
                "Choose a repository, then what to review in it.".into(),
            ),
            Some(source) => {
                let s = source.read(cx);
                let place = s
                    .worktree()
                    .map(tildify)
                    .unwrap_or_else(|| tildify(&s.repo().common_dir));
                (s.name().to_owned().into(), place.into())
            }
        };
        h_flex()
            .gap_2()
            .items_center()
            .when(self.source.is_some(), |d| {
                d.child(
                    Button::new("open-flow-back")
                        .icon(IconName::ChevronLeft)
                        .ghost()
                        .small()
                        .tooltip("Back to repositories (⌘[)")
                        .on_click(cx.listener(|flow, _, window, cx| flow.back(window, cx))),
                )
            })
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(subtitle),
                    ),
            )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Key caps like gpui-kit's `Kbd`; Escape reads "esc" (its ⎋ glyph
        // is easy to mistake for a reload arrow).
        let cap = |key: String| {
            div()
                .px_1()
                .py_0p5()
                .min_w(px(18.))
                .flex()
                .justify_center()
                .rounded(theme.radius)
                .bg(theme.muted)
                .text_color(theme.muted_foreground)
                .child(key)
        };
        let hint = |keys: &str, text: &'static str| {
            h_flex()
                .gap_1()
                .items_center()
                .children(keys.split_whitespace().filter_map(|k| {
                    let label = if k == "escape" {
                        "esc".to_owned()
                    } else {
                        Kbd::format(&Keystroke::parse(k).ok()?)
                    };
                    Some(cap(label))
                }))
                .child(div().pl_0p5().child(text))
        };
        let status: Option<(SharedString, bool)> = if let Some(error) = &self.error {
            Some((error.clone(), true))
        } else {
            self.choosing
                .as_ref()
                .map(|(path, _)| (format!("Reading {}…", tildify(path)).into(), false))
        };
        let left = match status {
            Some((text, is_error)) => h_flex()
                .gap_2()
                .min_w_0()
                .when(!is_error, |d| d.child(Spinner::new().small()))
                .child(
                    div()
                        .truncate()
                        .text_color(if is_error {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(text),
                )
                .into_any_element(),
            None => match &self.source {
                None => h_flex()
                    .gap_3()
                    .child(hint("up down", "select"))
                    .child(hint("enter", "choose"))
                    .child(hint("escape", "close"))
                    .into_any_element(),
                Some(_) => h_flex()
                    .gap_3()
                    .child(hint("cmd-1 cmd-2 cmd-3", "source"))
                    .child(hint("cmd-[", "back"))
                    .into_any_element(),
            },
        };
        let ready = self
            .source
            .as_ref()
            .is_some_and(|s| s.read(cx).request(cx).is_ok());
        h_flex()
            .gap_3()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().flex_1().min_w_0().child(left))
            .when(self.source.is_some(), |d| {
                d.child(
                    Button::new("open-flow-open")
                        .primary()
                        .small()
                        .label("Open")
                        .disabled(!ready)
                        .child(
                            h_flex()
                                .pl_1()
                                .gap_0p5()
                                .children(Keystroke::parse("cmd-enter").ok().map(Kbd::new)),
                        )
                        .on_click(cx.listener(|flow, _, window, cx| flow.confirm(window, cx))),
                )
            })
    }
}

impl Focusable for OpenFlow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for OpenFlow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.source {
            None => div()
                .size_full()
                .border_1()
                .border_color(cx.theme().border)
                .rounded(cx.theme().radius_lg)
                .overflow_hidden()
                .child(List::new(&self.repos).search_placeholder("Search recent repositories…"))
                .into_any_element(),
            Some(source) => source.clone().into_any_element(),
        };
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .debug_selector(|| "open-flow".into())
            .on_action(cx.listener(|flow, _: &Back, window, cx| flow.back(window, cx)))
            .on_action(cx.listener(|flow, _: &Open, window, cx| flow.confirm(window, cx)))
            .on_action(cx.listener(|flow, _: &ModeLive, window, cx| {
                flow.set_mode(SourceMode::Live, window, cx)
            }))
            .on_action(cx.listener(|flow, _: &ModeCommit, window, cx| {
                flow.set_mode(SourceMode::Commit, window, cx)
            }))
            .on_action(cx.listener(|flow, _: &ModeCompare, window, cx| {
                flow.set_mode(SourceMode::Compare, window, cx)
            }))
            .w_full()
            .h(px(HEIGHT))
            .gap_3()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(body))
            .child(self.render_footer(cx))
    }
}
