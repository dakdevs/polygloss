//! Step 2 of the open flow (design §3, §11.3): what to review in the chosen
//! repo.
//!
//! - **Working tree** (Live): the worktree against a base, since the merge
//!   base with the default branch (the default) or since HEAD. Each base is
//!   its own review (design §4.2).
//! - **Commit**: HEAD's log, newest first, in a virtualized gpui-kit list that
//!   reads [`COMMIT_PAGE`] commits at a time as you scroll
//!   (`listing::list_commits`), fuzzy-filtered with nucleo over subject,
//!   author and id; a query that is the start of an id puts that commit
//!   first.
//! - **Compare**: base and head pickers over every branch, remote branch and
//!   tag (`listing::list_refs`, fuzzy-searchable), three-dot by default with a
//!   direct (two-dot) toggle and an optional label such as `PR #123`. The base
//!   defaults to the default branch, the head to the branch checked out (or
//!   the newest other branch).
//!
//! [`SourceStep::request`] is the `OpenRequest` the choices make, or why
//! there is none yet; the flow opens it on ⌘⏎, Enter in the commit list or
//! Enter in the label field ([`SourceEvent::Confirm`]).

use std::path::Path;

use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::searchable_list::{SearchableListDelegate, SearchableListItem};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Selectable as _, Sizable as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};
use polygloss_core::git::listing::{CommitInfo, RefInfo, RefKind, list_commits, list_refs};
use polygloss_core::git::{
    CompareMode, Git, GitError, RepoInfo, Since, Source, default_branch, discover,
};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::{Actor, now_ms};

use crate::home::row::{local_utc_offset_s, relative_time};
use crate::open_flow::ranking::Ranker;
use crate::open_flow::repo_step::tildify;

/// How many commits the commit list reads at a time.
pub const COMMIT_PAGE: u32 = 200;

/// While a query is typed, the list keeps reading pages as you scroll only
/// up to this many commits (each page walks the history from HEAD again).
pub const SEARCH_LOAD_CAP: usize = 5_000;

const COMMIT_ROW_HEIGHT: f32 = 48.;

/// The three kinds of source (design §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    Live,
    Commit,
    Compare,
}

impl SourceMode {
    pub const ALL: [SourceMode; 3] = [SourceMode::Live, SourceMode::Commit, SourceMode::Compare];

    pub fn title(self) -> &'static str {
        match self {
            SourceMode::Live => "Working tree",
            SourceMode::Commit => "Commit",
            SourceMode::Compare => "Compare",
        }
    }
}

/// Emitted when the user asks to open what is chosen (Enter in the commit
/// list or the label field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEvent {
    Confirm,
}

/// What the flow reads about a chosen repo before showing this step (on the
/// background executor: [`load_repo`]).
#[derive(Debug, Clone)]
pub struct RepoData {
    pub repo: RepoInfo,
    /// The display name: the worktree's (or bare repo's) directory.
    pub name: String,
    /// Every branch, remote branch and tag.
    pub refs: Vec<RefInfo>,
    /// The default branch (full name), if there is one (design §3, OQ-5).
    pub default_branch: Option<String>,
    /// The first page of HEAD's log.
    pub commits: Vec<CommitInfo>,
}

/// Reads what the source step shows about the repo containing `path`.
/// `Err` is the message to show (not a repository, git failed).
pub fn load_repo(path: &Path) -> Result<RepoData, String> {
    let repo = discover(path).map_err(|e| match e {
        GitError::NotARepo(_) => format!("{} is not a git repository", tildify(path)),
        e => format!("Could not read {}: {e}", tildify(path)),
    })?;
    let git = git_for(&repo);
    let refs = list_refs(&git).map_err(|e| format!("Could not list the refs: {e}"))?;
    let default_branch = default_branch(&git).ok().map(|(name, _)| name);
    let commits =
        list_commits(&git, 0, COMMIT_PAGE).map_err(|e| format!("Could not read the log: {e}"))?;
    let dir = repo
        .toplevel
        .clone()
        .unwrap_or_else(|| repo.common_dir.clone());
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string());
    Ok(RepoData {
        repo,
        name,
        refs,
        default_branch,
        commits,
    })
}

fn git_for(repo: &RepoInfo) -> Git {
    Git::new(
        repo.toplevel
            .clone()
            .unwrap_or_else(|| repo.git_dir.clone()),
    )
}

// ---------------------------------------------------------------- commits

/// The commit list's delegate: the pages read so far and the fuzzy matches.
pub struct CommitDelegate {
    git: Git,
    commits: Vec<CommitInfo>,
    /// What the ranker matches per commit: subject, author and id.
    haystacks: Vec<String>,
    /// Indices into `commits`, best first.
    matches: Vec<usize>,
    query: String,
    ranker: Ranker,
    /// The selected row (an index into `matches`).
    selected: Option<usize>,
    exhausted: bool,
    loading: Option<Task<()>>,
    now_ms: i64,
    utc_offset_s: i64,
}

impl CommitDelegate {
    fn new(git: Git, first_page: Vec<CommitInfo>) -> CommitDelegate {
        let now = now_ms();
        let mut delegate = CommitDelegate {
            git,
            commits: Vec::new(),
            haystacks: Vec::new(),
            matches: Vec::new(),
            query: String::new(),
            ranker: Ranker::text(),
            selected: None,
            exhausted: false,
            loading: None,
            now_ms: now,
            utc_offset_s: local_utc_offset_s(now),
        };
        delegate.append(Ok(first_page));
        delegate
    }

    /// Every commit read so far, newest first.
    pub fn loaded(&self) -> &[CommitInfo] {
        &self.commits
    }

    /// Reads relative dates as seen at `now_ms` (Unix ms) in the time zone
    /// `utc_offset_s` seconds east of UTC (screenshots pin both).
    pub fn set_clock(&mut self, now_ms: i64, utc_offset_s: i64) {
        self.now_ms = now_ms;
        self.utc_offset_s = utc_offset_s;
    }

    /// Whether the whole log has been read.
    pub fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// The commits matching the query, best first.
    pub fn matches(&self) -> impl Iterator<Item = &CommitInfo> {
        self.matches.iter().map(|&ix| &self.commits[ix])
    }

    /// The selected commit.
    pub fn selected(&self) -> Option<&CommitInfo> {
        let ix = *self.matches.get(self.selected?)?;
        self.commits.get(ix)
    }

    fn append(&mut self, page: Result<Vec<CommitInfo>, GitError>) {
        self.loading = None;
        match page {
            Ok(page) => {
                self.exhausted = page.len() < COMMIT_PAGE as usize;
                self.haystacks.extend(
                    page.iter()
                        .map(|c| format!("{}  {}  {}", c.subject, c.author, c.oid)),
                );
                self.commits.extend(page);
            }
            Err(e) => {
                tracing::warn!("reading the log: {e}");
                self.exhausted = true;
            }
        }
        self.rerank();
    }

    fn rerank(&mut self) {
        let mut ranked = self.ranker.rank(&self.query, &self.haystacks);
        // A query that starts an id (`1a2b3c4`) finds that commit first.
        let q = self.query.trim().to_ascii_lowercase();
        if q.len() >= 4 && q.bytes().all(|b| b.is_ascii_hexdigit()) {
            let exact: Vec<usize> = (0..self.commits.len())
                .filter(|&ix| self.commits[ix].oid.as_str().starts_with(&q))
                .collect();
            ranked.retain(|ix| !exact.contains(ix));
            ranked.splice(0..0, exact);
        }
        self.matches = ranked;
        if self.selected.is_some_and(|s| s >= self.matches.len()) {
            self.selected = (!self.matches.is_empty()).then_some(0);
        }
    }

    fn can_load_more(&self) -> bool {
        !self.exhausted
            && self.loading.is_none()
            && (self.query.trim().is_empty() || self.commits.len() < SEARCH_LOAD_CAP)
    }
}

impl ListDelegate for CommitDelegate {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_owned();
        self.rerank();
        Task::ready(())
    }

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.matches.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let commit = self.commits.get(*self.matches.get(ix.row)?)?;
        let theme = cx.theme();
        let when = relative_time(self.now_ms, commit.authored_at * 1000, self.utc_offset_s);
        let row = ix.row;
        Some(
            ListItem::new(("open-flow-commit", ix.row))
                .h(px(COMMIT_ROW_HEIGHT))
                .rounded(theme.radius)
                .child(
                    h_flex()
                        .debug_selector(move || format!("open-flow-commit-{row}"))
                        .gap_3()
                        .min_w_0()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(commit.subject.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{} committed {when}", commit.author)),
                                ),
                        )
                        .child(sha_pill(commit.oid.short(), cx)),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let text = if self.commits.is_empty() {
            "This branch has no commits yet"
        } else {
            "No matching commits"
        };
        div()
            .py_6()
            .w_full()
            .flex()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(text)
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix.map(|ix| ix.row);
    }

    fn has_more(&self, _cx: &App) -> bool {
        self.can_load_more()
    }

    fn load_more_threshold(&self) -> usize {
        40
    }

    fn load_more(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        if !self.can_load_more() {
            return;
        }
        let git = self.git.clone();
        let skip = self.commits.len() as u32;
        self.loading = Some(cx.spawn_in(window, async move |list, cx| {
            let page = cx
                .background_spawn(async move { list_commits(&git, skip, COMMIT_PAGE) })
                .await;
            list.update_in(cx, |list, _, cx| {
                list.delegate_mut().append(page);
                cx.notify();
            })
            .ok();
        }));
    }
}

/// A short commit id in the code font, in a bordered pill (the commit
/// lists).
pub fn sha_pill(short: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(short.to_owned())
}

// ---------------------------------------------------------------- refs

/// One ref in a base or head picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefItem {
    pub info: RefInfo,
}

impl SearchableListItem for RefItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.info.short_name.clone().into()
    }

    fn value(&self) -> &String {
        &self.info.name
    }

    fn render(&self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let kind = match self.info.kind {
            RefKind::Branch => "branch",
            RefKind::RemoteBranch => "remote",
            RefKind::Tag => "tag",
        };
        h_flex()
            .w_full()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .w(px(52.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(kind),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(self.info.short_name.clone()),
            )
            .when(self.info.is_head, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("HEAD"),
                )
            })
            .child(
                div()
                    .flex_none()
                    .font_family(theme.mono_font_family.clone())
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.info.commit.short().to_owned()),
            )
    }
}

/// A ref picker's delegate: every ref, fuzzy-filtered with nucleo.
pub struct RefDelegate {
    items: Vec<RefItem>,
    haystacks: Vec<String>,
    matches: Vec<usize>,
    ranker: Ranker,
}

impl RefDelegate {
    fn new(refs: &[RefInfo]) -> RefDelegate {
        RefDelegate {
            items: refs.iter().cloned().map(|info| RefItem { info }).collect(),
            haystacks: refs.iter().map(|r| r.short_name.clone()).collect(),
            matches: (0..refs.len()).collect(),
            ranker: Ranker::paths(),
        }
    }
}

impl SearchableListDelegate for RefDelegate {
    type Item = RefItem;

    fn items_count(&self, _section: usize) -> usize {
        self.matches.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&RefItem> {
        self.items.get(*self.matches.get(ix.row)?)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        RefItem: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matches
            .iter()
            .position(|&ix| self.items[ix].value() == value)
            .map(IndexPath::new)
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _cx: &mut App) -> Task<()> {
        self.matches = self.ranker.rank(query, &self.haystacks);
        Task::ready(())
    }
}

/// The compare defaults: the default branch as base; the branch checked
/// out as head, or the newest other branch when that is the base.
pub fn default_compare(
    refs: &[RefInfo],
    default: Option<&str>,
) -> (Option<String>, Option<String>) {
    let base = default
        .filter(|d| refs.iter().any(|r| r.name == *d))
        .map(str::to_owned);
    let branches = || refs.iter().filter(|r| r.kind == RefKind::Branch);
    let head = branches()
        .find(|r| r.is_head && Some(&r.name) != base.as_ref())
        .or_else(|| branches().find(|r| Some(&r.name) != base.as_ref()))
        .map(|r| r.name.clone());
    (base, head)
}

// ---------------------------------------------------------------- the step

/// Step 2: the source.
pub struct SourceStep {
    data: RepoData,
    mode: SourceMode,
    since: Since,
    commits: Entity<ListState<CommitDelegate>>,
    base: Entity<SelectState<RefDelegate>>,
    head: Entity<SelectState<RefDelegate>>,
    direct: bool,
    label: Entity<InputState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SourceEvent> for SourceStep {}

impl SourceStep {
    pub fn new(data: RepoData, window: &mut Window, cx: &mut Context<SourceStep>) -> SourceStep {
        let git = git_for(&data.repo);
        let first_page = data.commits.clone();
        let commits = cx.new(|cx| {
            let mut list =
                ListState::new(CommitDelegate::new(git, first_page), window, cx).searchable(true);
            if !list.delegate().matches.is_empty() {
                list.set_selected_index(Some(IndexPath::default()), window, cx);
            }
            list
        });
        let (base_ref, head_ref) = default_compare(&data.refs, data.default_branch.as_deref());
        let picker =
            |selected: Option<String>, window: &mut Window, cx: &mut Context<SourceStep>| {
                let delegate = RefDelegate::new(&data.refs);
                let ix = selected.and_then(|name| delegate.position(&name));
                cx.new(|cx| SelectState::new(delegate, ix, window, cx).searchable(true))
            };
        let base = picker(base_ref, window, cx);
        let head = picker(head_ref, window, cx);
        let label =
            cx.new(|cx| InputState::new(window, cx).placeholder("Label (optional), e.g. PR #123"));
        let subscriptions = vec![
            cx.subscribe(&commits, |_, _, event: &ListEvent, cx| {
                if let ListEvent::Confirm(_) = event {
                    cx.emit(SourceEvent::Confirm);
                }
            }),
            cx.subscribe(&label, |_, _, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    cx.emit(SourceEvent::Confirm);
                }
            }),
        ];
        let mode = if data.repo.toplevel.is_some() {
            SourceMode::Live
        } else {
            SourceMode::Commit
        };
        SourceStep {
            data,
            mode,
            since: Since::MergeBase,
            commits,
            base,
            head,
            direct: false,
            label,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn repo(&self) -> &RepoInfo {
        &self.data.repo
    }

    /// The repo's name.
    pub fn name(&self) -> &str {
        &self.data.name
    }

    /// The worktree a live review watches (`None` for a bare repo).
    pub fn worktree(&self) -> Option<&Path> {
        self.data.repo.toplevel.as_deref()
    }

    pub fn mode(&self) -> SourceMode {
        self.mode
    }

    /// Switches the source kind and moves focus to its first control.
    pub fn set_mode(&mut self, mode: SourceMode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.focus_mode(window, cx);
        cx.notify();
    }

    /// Focuses the current mode's first control: the step itself (Live),
    /// the commit search (Commit) or the label field (Compare).
    pub fn focus_mode(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            SourceMode::Live => window.focus(&self.focus, cx),
            SourceMode::Commit => self.commits.update(cx, |l, cx| l.focus(window, cx)),
            SourceMode::Compare => self.focus_label(window, cx),
        }
    }

    /// The live base.
    pub fn since(&self) -> Since {
        self.since.clone()
    }

    pub fn set_since(&mut self, since: Since, cx: &mut Context<Self>) {
        self.since = since;
        cx.notify();
    }

    /// The commit list.
    pub fn commits(&self) -> &Entity<ListState<CommitDelegate>> {
        &self.commits
    }

    /// The chosen base ref (full name).
    pub fn base(&self, cx: &App) -> Option<String> {
        self.base.read(cx).selected_value().cloned()
    }

    /// The chosen head ref (full name).
    pub fn head(&self, cx: &App) -> Option<String> {
        self.head.read(cx).selected_value().cloned()
    }

    pub fn set_base(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.base
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
        cx.notify();
    }

    pub fn set_head(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.head
            .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
        cx.notify();
    }

    /// Every ref the pickers offer, in their order.
    pub fn ref_names(&self) -> Vec<String> {
        self.data.refs.iter().map(|r| r.name.clone()).collect()
    }

    /// Direct (two-dot) instead of three-dot.
    pub fn direct(&self) -> bool {
        self.direct
    }

    pub fn set_direct(&mut self, direct: bool, cx: &mut Context<Self>) {
        self.direct = direct;
        cx.notify();
    }

    /// The label as typed.
    pub fn label(&self, cx: &App) -> String {
        self.label.read(cx).value().to_string()
    }

    pub fn set_label(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let label = label.to_owned();
        self.label
            .update(cx, |s, cx| s.set_value(label, window, cx));
    }

    /// The label field.
    pub fn label_input(&self) -> &Entity<InputState> {
        &self.label
    }

    pub fn focus_label(&self, window: &mut Window, cx: &mut App) {
        let handle = self.label.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// What the choices open, or why they do not open anything yet.
    pub fn request(&self, cx: &App) -> Result<OpenRequest, SharedString> {
        let repo = &self.data.repo;
        let worktree = repo
            .toplevel
            .clone()
            .unwrap_or_else(|| repo.common_dir.clone());
        let (source, label) = match self.mode {
            SourceMode::Live => {
                if repo.toplevel.is_none() {
                    return Err("A bare repository has no working tree".into());
                }
                (
                    Source::Live {
                        since: self.since.clone(),
                    },
                    None,
                )
            }
            SourceMode::Commit => {
                let list = self.commits.read(cx);
                let commit = list
                    .delegate()
                    .selected()
                    .ok_or_else(|| SharedString::from("Choose a commit"))?;
                (
                    Source::Commit {
                        rev: commit.oid.as_str().to_owned(),
                    },
                    None,
                )
            }
            SourceMode::Compare => {
                let (Some(base), Some(head)) = (self.base(cx), self.head(cx)) else {
                    return Err("Choose a base and a head to compare".into());
                };
                if base == head {
                    return Err("Choose two different refs to compare".into());
                }
                let mode = if self.direct {
                    CompareMode::Direct
                } else {
                    CompareMode::ThreeDot
                };
                let label = self.label(cx).trim().to_owned();
                (
                    Source::Compare { base, head, mode },
                    (!label.is_empty()).then_some(label),
                )
            }
        };
        Ok(OpenRequest {
            worktree,
            source,
            label,
            pin: None,
            actor: Actor::human(),
        })
    }

    fn head_branch(&self) -> Option<&RefInfo> {
        self.data.refs.iter().find(|r| r.is_head)
    }

    fn render_modes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let live_ok = self.data.repo.toplevel.is_some();
        let mut group = ButtonGroup::new("open-flow-mode").outline().small();
        for (ix, mode) in SourceMode::ALL.into_iter().enumerate() {
            group = group.child(
                Button::new(("open-flow-mode", ix))
                    .label(mode.title())
                    .selected(self.mode == mode)
                    .disabled(mode == SourceMode::Live && !live_ok)
                    .tooltip(format!("{} (⌘{})", mode.title(), ix + 1)),
            );
        }
        group.on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
            if let Some(&ix) = clicked.first() {
                this.set_mode(SourceMode::ALL[ix], window, cx);
            }
        }))
    }

    fn render_live(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.data.repo.toplevel.is_none() {
            return div()
                .py_6()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("A bare repository has no working tree to review.")
                .into_any_element();
        }
        let branch = self
            .head_branch()
            .map(|r| r.short_name.clone())
            .unwrap_or_else(|| "a detached HEAD".to_owned());
        let default = self
            .data
            .default_branch
            .as_deref()
            .map(|d| crate::review_tab::short_ref(d).to_owned());
        let merge_base_text = match &default {
            Some(d) => {
                format!("Everything on {branch} that is not on {d} yet, plus uncommitted changes.")
            }
            None => "No default branch was found, so this is the same as since HEAD.".to_owned(),
        };
        let option = |id: &'static str,
                      since: Since,
                      title: String,
                      text: String,
                      cx: &mut Context<Self>| {
            let selected = self.since == since;
            h_flex()
                .id(id)
                .debug_selector(move || id.to_owned())
                .gap_3()
                .items_start()
                .p_3()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(if selected {
                    theme.primary
                } else {
                    theme.border
                })
                .when(selected, |d| d.bg(theme.primary.opacity(0.06)))
                .cursor_pointer()
                .hover(|d| d.bg(theme.list_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.set_since(since.clone(), cx)))
                .child(
                    div()
                        .flex_none()
                        .mt(px(2.))
                        .size(px(14.))
                        .rounded_full()
                        .border_1()
                        .border_color(if selected { theme.primary } else { theme.input })
                        .when(selected, |d| {
                            d.p(px(3.))
                                .child(div().size_full().rounded_full().bg(theme.primary))
                        }),
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .min_w_0()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(text),
                        ),
                )
        };
        v_flex()
            .gap_2()
            .child(
                div()
                    .pb_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "Review the working tree of {} on {branch}, including new files.",
                        tildify(self.data.repo.toplevel.as_deref().unwrap_or(Path::new("")))
                    )),
            )
            .child(option(
                "open-flow-since-merge-base",
                Since::MergeBase,
                match &default {
                    Some(d) => format!("Since the merge base with {d}"),
                    None => "Since the merge base".to_owned(),
                },
                merge_base_text,
                cx,
            ))
            .child(option(
                "open-flow-since-head",
                Since::Head,
                "Since HEAD".to_owned(),
                "Only the uncommitted changes, staged or not.".to_owned(),
                cx,
            ))
            .into_any_element()
    }

    fn render_compare(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let field = |title: &'static str, select: AnyElement| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(title),
                )
                .child(select)
        };
        let picker = |state: &Entity<SelectState<RefDelegate>>, id: &'static str| {
            div()
                .debug_selector(move || id.to_owned())
                .child(
                    Select::new(state)
                        .placeholder("Choose a branch or tag")
                        .search_placeholder("Search branches and tags…")
                        .menu_max_h(px(260.)),
                )
                .into_any_element()
        };
        let this = cx.entity().downgrade();
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(field("Base", picker(&self.base, "open-flow-base")))
                    .child(
                        div().flex_none().pb_2().child(
                            Icon::new(IconName::ArrowLeft)
                                .small()
                                .text_color(theme.muted_foreground),
                        ),
                    )
                    .child(field("Compare", picker(&self.head, "open-flow-head"))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        Switch::new("open-flow-direct")
                            .checked(self.direct)
                            .small()
                            .on_click(move |checked, _, cx| {
                                this.update(cx, |s, cx| s.set_direct(*checked, cx)).ok();
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Direct comparison"),
                            )
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                if self.direct {
                                    "Base's tree against the compare's tree (two dots)."
                                } else {
                                    "Changes on the compare side since the merge base (three \
                                     dots), as in a pull request."
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.muted_foreground)
                            .child("Label"),
                    )
                    .child(Input::new(&self.label)),
            )
            .into_any_element()
    }
}

impl Focusable for SourceStep {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SourceStep {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.mode {
            SourceMode::Live => self.render_live(cx),
            SourceMode::Commit => List::new(&self.commits)
                .search_placeholder("Search commits by message, author or id…")
                .into_any_element(),
            SourceMode::Compare => self.render_compare(cx),
        };
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .gap_3()
            .child(self.render_modes(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.mode == SourceMode::Commit, |d| {
                        d.border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius_lg)
                            .overflow_hidden()
                    })
                    .child(body),
            )
    }
}
