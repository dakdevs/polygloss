//! A review tab (design §11.1, ADR-0026): the window's shell
//! ([`crate::chrome::shell`]) with the sidebar (its top row, then the file
//! tree, or find in its place, or the Reviews list) beside the main column
//! (toolbar row, banner strip, then the diff viewport | threads panel). One
//! tab per review; opening a review that is already open focuses its tab
//! ([`open_review`]). The sidebar renders inside the tab, so the tree keeps
//! the tab's key context.
//!
//! Features plug in without editing this module: every feature module's
//! `attach` runs for each new tab (subscriptions, per-tab state kept with
//! [`ReviewTab::insert_extension`]), the toolbar calls their
//! `toolbar_items`, the panes call `tree::render_pane` and
//! `threads::render_panel`, and banners go through [`ReviewTab::banners`].

pub mod banners;
pub mod panes;
pub mod toolbar;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
};
use polygloss_core::git::ReviewKind;
use polygloss_core::review::{Core, CoreError, OpenRequest, OpenedDiff};
use polygloss_viewport::{DiffProvider, DiffViewport, ViewportEvent, ViewportOptions};

pub use banners::{BannerKind, BannerStrip};

use crate::app_state::AppState;
use crate::provider::CoreDiffProvider;
use crate::settings::{Settings, SettingsStore};
use crate::window::MainWindow;

/// Registers the review tab's actions.
pub fn init(_cx: &mut gpui_kit::App) {}

/// One open review.
pub struct ReviewTab {
    pub review_id: String,
    pub opened: OpenedDiff,
    pub viewport: Entity<DiffViewport>,
    pub banners: Entity<BannerStrip>,
    pub(crate) panes: panes::Panes,
    focus: FocusHandle,
    /// The viewport pane's focus (key context `Viewport`), where the tab's
    /// keyboard focus goes when it is activated.
    viewport_focus: FocusHandle,
    /// The settings the viewport options were last built from.
    applied: Arc<Settings>,
    extensions: HashMap<TypeId, Box<dyn Any>>,
    _subscriptions: Vec<Subscription>,
}

/// The viewport options for review tabs: the settings, with the theme of
/// [`crate::theme::viewport_theme`].
pub fn viewport_options(settings: &Settings, cx: &App) -> ViewportOptions {
    ViewportOptions {
        theme: crate::theme::viewport_theme(cx),
        ..settings.viewport_options()
    }
}

impl ReviewTab {
    /// A tab showing `opened`, reading blobs from `provider`.
    pub fn new(
        opened: OpenedDiff,
        provider: Arc<dyn DiffProvider>,
        window: &mut Window,
        cx: &mut Context<ReviewTab>,
    ) -> ReviewTab {
        let settings = SettingsStore::global(cx).shared();
        let opts = viewport_options(&settings, cx);
        let viewport = cx.new(|cx| DiffViewport::new(provider, opts, window, cx));
        let focus = cx.focus_handle();
        let banners =
            cx.new(|_| BannerStrip::new(description(&opened).into()).with_target(focus.clone()));
        let core = AppState::global(cx).core.clone();
        let subscriptions = vec![
            // The diff shown now: a refresh (T3.11) swaps it in the same
            // viewport.
            cx.subscribe(
                &viewport,
                move |tab: &mut ReviewTab, _, event: &ViewportEvent, cx| {
                    if let ViewportEvent::BinaryDetected(idx) = *event {
                        write_binary_kind(&core, &tab.opened.diff_id, idx, cx);
                    }
                },
            ),
            cx.observe_global::<SettingsStore>(|tab: &mut ReviewTab, cx| tab.apply_settings(cx)),
        ];
        ReviewTab {
            review_id: opened.review_id.clone(),
            opened,
            viewport,
            banners,
            panes: panes::Panes::new(cx),
            focus,
            viewport_focus: cx.focus_handle(),
            applied: settings,
            extensions: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The tab's label: the repo and what is compared (`app · main...topic`).
    pub fn title(&self) -> SharedString {
        title(&self.opened).into()
    }

    /// The viewport pane's focus handle (key context `Viewport`). Actions
    /// dispatched on it reach the viewport's handlers and then the tab's.
    pub fn viewport_focus(&self) -> &FocusHandle {
        &self.viewport_focus
    }

    /// The viewport options for `settings`, with this tab's view toggles
    /// (split/unified, whitespace, word diff; `palette::view_toggles`) on
    /// top.
    pub fn options_for(&self, settings: &Settings, cx: &App) -> ViewportOptions {
        let mut opts = viewport_options(settings, cx);
        crate::palette::view_toggles::apply_overrides(self, &mut opts);
        opts
    }

    pub fn threads_panel_visible(&self) -> bool {
        self.panes.threads_visible
    }

    pub fn toggle_threads_panel(&mut self, cx: &mut Context<Self>) {
        self.panes.threads_visible = !self.panes.threads_visible;
        cx.notify();
    }

    /// Keeps a feature's per-tab state (one value per type).
    pub fn insert_extension<T: 'static>(&mut self, value: T) {
        self.extensions.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn extension<T: 'static>(&self) -> Option<&T> {
        self.extensions.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn extension_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.extensions.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    /// Re-applies the settings to the viewport when they changed.
    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let settings = SettingsStore::global(cx).shared();
        if Arc::ptr_eq(&settings, &self.applied) || *settings == *self.applied {
            return;
        }
        let opts = self.options_for(&settings, cx);
        self.applied = settings;
        self.viewport.update(cx, |v, cx| v.set_options(opts, cx));
    }
}

/// Stores that file `idx` of `diff_id` is binary (T1.3's first-read rule),
/// off the main thread.
fn write_binary_kind(
    core: &Core,
    diff_id: &polygloss_core::ids::DiffId,
    idx: u32,
    cx: &mut Context<ReviewTab>,
) {
    let (core, diff_id) = (core.clone(), diff_id.clone());
    cx.background_spawn(async move {
        if let Err(e) = core.mark_file_binary(&diff_id, idx) {
            tracing::warn!("storing file {idx} of {diff_id} as binary: {e}");
        }
    })
    .detach();
}

/// Activating the tab focuses its viewport; the tab's own handle (its root,
/// key context `Tab`) is the banner strip's dispatch target.
impl Focusable for ReviewTab {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.viewport_focus.clone()
    }
}

impl Render for ReviewTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = panes::render_sidebar(self, window, cx);
        let toolbar = toolbar::render(self, window, cx);
        let panes = panes::render(self, window, cx);
        let main = v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(toolbar)
            .child(self.banners.clone())
            .child(gpui_kit::div().flex_1().min_h_0().child(panes))
            .into_any_element();
        let shell = crate::chrome::shell(sidebar, main, self.panes.threads_visible, window, cx);
        crate::keymap::handlers::apply(v_flex(), cx)
            .key_context("Tab")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|tab, _: &panes::ToggleThreadsPanel, _, cx| {
                    tab.toggle_threads_panel(cx)
                }),
            )
            .size_full()
            .child(shell)
    }
}

/// The repo's name: its worktree's directory, else the git dir's parent.
fn repo_name(opened: &OpenedDiff) -> String {
    let dir = opened.repo.toplevel.clone().unwrap_or_else(|| {
        let common = &opened.repo.common_dir;
        common.parent().unwrap_or(common).to_path_buf()
    });
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

/// A ref as people write it: `refs/heads/x` → `x`, `refs/remotes/o/x` →
/// `o/x`, `refs/tags/v1` → `v1`, a full commit id → its first 7 digits.
pub fn short_ref(r: &str) -> &str {
    let name = ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|p| r.strip_prefix(p))
        .unwrap_or(r);
    if name.len() >= 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
        &name[..7]
    } else {
        name
    }
}

/// A compare review's `(separator, base, head)` from its key
/// (`compare:<base>...<head>` or `compare:<base>..<head>`, design §4.2),
/// with the refs as the key holds them. Git ref names never contain `..`,
/// so the first `...` (else `..`) is the separator, whatever dots the refs
/// hold (`v1.2.0`, `release-1.2`).
fn compare_sides(key: &str) -> (&'static str, &str, &str) {
    let spec = key.strip_prefix("compare:").unwrap_or(key);
    match spec.split_once("...") {
        Some((base, head)) => ("...", base, head),
        None => match spec.split_once("..") {
            Some((base, head)) => ("..", base, head),
            None => ("..", spec, ""),
        },
    }
}

/// A live review's branch from its key
/// (`worktree:<worktree>@<branch>#since=<since>`, design §4.2). Branch
/// names may hold `@` and `#`, and so may the worktree path, so the
/// worktree prefix is removed by value when known and only the `#since=`
/// suffix (whose value is `merge-base`, `HEAD` or an OID) is cut from the
/// end.
fn live_branch(opened: &OpenedDiff) -> Option<&str> {
    let rest = opened.review_key.strip_prefix("worktree:")?;
    let rest = rest.rsplit_once("#since=").map_or(rest, |(r, _)| r);
    let worktrees = opened
        .live
        .as_ref()
        .map(|l| l.worktree.as_path())
        .into_iter()
        .chain(opened.repo.toplevel.as_deref());
    for worktree in worktrees {
        let prefix = format!("{}@", worktree.display());
        if let Some(branch) = rest.strip_prefix(prefix.as_str()) {
            return Some(branch);
        }
    }
    rest.split_once('@').map(|(_, branch)| branch)
}

/// What the review compares, from its key (design §4.2).
fn source_summary(opened: &OpenedDiff) -> String {
    match opened.kind {
        ReviewKind::Compare => {
            let (sep, base, head) = compare_sides(&opened.review_key);
            format!("{}{sep}{}", short_ref(base), short_ref(head))
        }
        ReviewKind::Commit => {
            let key = opened.review_key.as_str();
            let oid = key.strip_prefix("commit:").unwrap_or(key);
            oid.chars().take(7).collect()
        }
        ReviewKind::Live => {
            let branch = live_branch(opened).unwrap_or("worktree");
            format!("{branch} (working tree)")
        }
    }
}

/// The tab label: `<repo> · <what>`.
pub fn title(opened: &OpenedDiff) -> String {
    format!("{} · {}", repo_name(opened), source_summary(opened))
}

/// The banner strip's line when no banner shows: both sides and the size.
pub fn description(opened: &OpenedDiff) -> String {
    let short = |oid: &polygloss_diff::Oid| oid.short().to_string();
    let files = match opened.files.len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let base = match (&opened.base.ref_name, &opened.base.commit) {
        (Some(r), Some(c)) => format!("{} ({})", short_ref(r), short(c)),
        (None, Some(c)) => short(c),
        (Some(r), None) => short_ref(r).to_owned(),
        (None, None) => format!("tree {}", short(&opened.base.tree)),
    };
    let head = match opened.kind {
        ReviewKind::Live => "the working tree".to_owned(),
        _ => opened
            .head_commit
            .as_ref()
            .map(short)
            .unwrap_or_else(|| format!("tree {}", short(&opened.head_tree))),
    };
    match opened.kind {
        ReviewKind::Commit => format!("{} · {files} changed against its parent", head),
        ReviewKind::Compare => {
            let (_, _, head_ref) = compare_sides(&opened.review_key);
            let head_name = short_ref(head_ref).to_owned();
            let head_side = if head_name.is_empty() || head_name == head {
                head
            } else {
                format!("{head_name} ({head})")
            };
            format!("{base} → {head_side} · {files} changed")
        }
        ReviewKind::Live => format!("{base} → {head} · {files} changed"),
    }
}

/// An [`on_new_tab`] hook.
type NewTabHook = Box<dyn Fn(&Entity<ReviewTab>, &mut Window, &mut App)>;

/// Hooks run for every new review tab (see [`on_new_tab`]).
#[derive(Default)]
struct NewTabHooks(Vec<NewTabHook>);

impl Global for NewTabHooks {}

/// Runs `hook` for every review tab created from now on, right after it is
/// created and before its first frame (the perf scenarios subscribe to the
/// viewport's frames this way).
pub fn on_new_tab(
    cx: &mut App,
    hook: impl Fn(&Entity<ReviewTab>, &mut Window, &mut App) + 'static,
) {
    cx.default_global::<NewTabHooks>().0.push(Box::new(hook));
}

/// The message shown when `err` keeps a review from opening. Missing
/// objects (design §5.3: history beyond a shallow clone, or collected
/// commits) say "objects no longer available".
pub fn open_error_message(err: &anyhow::Error) -> String {
    let missing = err.chain().any(|e| {
        e.downcast_ref::<CoreError>()
            .is_some_and(|e| e.code() == "objects_missing")
            || e.downcast_ref::<polygloss_core::objects::ObjectError>()
                .is_some_and(|e| matches!(e, polygloss_core::objects::ObjectError::Missing(_)))
    });
    if missing {
        format!("Could not open the review: objects no longer available ({err:#})")
    } else {
        format!("Could not open the review: {err:#}")
    }
}

/// An open in flight: `Core::open` and the blob reader, on the background
/// executor.
pub type Opening = Task<anyhow::Result<(OpenedDiff, CoreDiffProvider)>>;

/// Starts opening `req` on the background executor (resolve, snapshot,
/// `diff-tree`, the store), so it runs while the window and its chrome come
/// up; [`finish_open`] shows the result.
pub fn start_open(req: OpenRequest, cx: &mut App) -> Opening {
    let core = AppState::global(cx).core.clone();
    cx.background_spawn(async move {
        let opened = core.open(&req)?;
        let provider = CoreDiffProvider::open(&opened)?;
        anyhow::Ok((opened, provider))
    })
}

/// Shows an open started by [`start_open`] in the main window once it is
/// done: a new tab, or the tab already showing that review, focused.
/// Errors are shown in the window and returned.
pub fn finish_open(
    opening: Opening,
    window: &mut Window,
    cx: &mut App,
) -> Task<anyhow::Result<Entity<ReviewTab>>> {
    window.spawn(cx, async move |cx| {
        let result = opening.await;
        cx.update(|window, cx| {
            let main = crate::window::main_window(cx)
                .map(|(_, main)| main)
                .context("the main window is closed")?;
            match result {
                Ok((opened, provider)) => Ok(main.update(cx, |main, cx| {
                    main.show_review(opened, Arc::new(provider), window, cx)
                })),
                Err(err) => {
                    let message = open_error_message(&err);
                    main.update(cx, |main, cx| main.show_error(message.clone(), window, cx));
                    Err(anyhow::anyhow!(message))
                }
            }
        })?
    })
}

/// Opens the review `req` names in the main window ([`start_open`], then
/// [`finish_open`]): `Core::open` runs on the background executor, never on
/// the UI thread; opening a review that is already open focuses its tab.
pub fn open_review(
    req: OpenRequest,
    window: &mut Window,
    cx: &mut App,
) -> Task<anyhow::Result<Entity<ReviewTab>>> {
    let opening = start_open(req, cx);
    finish_open(opening, window, cx)
}

impl MainWindow {
    /// Focuses the tab of `opened.review_id`, or adds one.
    pub fn show_review(
        &mut self,
        opened: OpenedDiff,
        provider: Arc<dyn DiffProvider>,
        window: &mut Window,
        cx: &mut Context<MainWindow>,
    ) -> Entity<ReviewTab> {
        if let Some(ix) = self.tabs().find_review(&opened.review_id, cx) {
            self.activate_tab(ix, window, cx);
            let tab = self
                .tabs()
                .get(ix)
                .and_then(|t| t.review())
                .cloned()
                .expect("a review tab");
            // The open produced another diff (the refs or the worktree
            // moved): the tab keeps what it shows and offers a refresh.
            tab.update(cx, |tab, cx| {
                crate::live::newer_diff_opened(tab, &opened, cx)
            });
            return tab;
        }
        for warning in &opened.warnings {
            tracing::info!("{}: {warning}", opened.review_key);
        }
        let tab = cx.new(|cx| ReviewTab::new(opened, provider, window, cx));
        tab.update(cx, |tab, cx| {
            crate::features::attach_review_tab(tab, window, cx)
        });
        run_new_tab_hooks(&tab, window, cx);
        let ix = self.push_review(tab.clone(), window, cx);
        debug_assert_eq!(self.tabs().active(), ix);
        tab
    }
}

fn run_new_tab_hooks(tab: &Entity<ReviewTab>, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<NewTabHooks>() {
        return;
    }
    // Taken out while they run, so a hook may add hooks.
    let hooks = std::mem::take(&mut cx.global_mut::<NewTabHooks>().0);
    for hook in &hooks {
        hook(tab, window, cx);
    }
    let all = &mut cx.global_mut::<NewTabHooks>().0;
    let added = std::mem::replace(all, hooks);
    all.extend(added);
}
