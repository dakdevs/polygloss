//! The base picker of a live review (design §10 "Base", §11.4, ADR-0008):
//! merge base (the default), HEAD, or a fixed commit from the log. Each
//! base is its own review key (`…#since=merge-base|HEAD|<oid>`, §4.2), so
//! choosing one opens that review in its own tab (or focuses it), and the
//! tab it was chosen from keeps showing its own base.
//!
//! `tab::ChooseBase` (the palette, the Review menu) and the toolbar's
//! Live pill ("Live · merge base") open a searchable list (a gpui-kit `Dialog` with a
//! `List`, like ⌘P): the two moving bases first, then the worktree's
//! commits (newest first, read in the background), ranked by
//! `nucleo-matcher` over subject, author and id. The current base is
//! checked; Enter chooses.

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Task, WeakEntity, Window, div, px,
};
use polygloss_core::git::listing::{CommitInfo, list_commits};
use polygloss_core::git::{Git, Since, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::{Actor, now_ms};

use crate::home::row::{local_utc_offset_s, relative_time};
use crate::open_flow::ranking::Ranker;
use crate::open_flow::repo_step::list_header;
use crate::open_flow::source_step::sha_pill;
use crate::review_tab::{ReviewTab, open_review};
use crate::space::{TextStyleExt as _, edge, gap, height, layout, pad, radius, size, text};

/// Commits listed (newest first).
pub const COMMITS: u32 = 500;

/// How a base reads in the toolbar's Live pill ("Live · merge base").
pub fn since_label(since: &Since) -> String {
    match since {
        Since::MergeBase => "merge base".to_owned(),
        Since::Head => "HEAD".to_owned(),
        Since::Commit(oid) => oid.chars().take(7).collect(),
    }
}

/// One row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseChoice {
    MergeBase,
    Head,
    Commit(CommitInfo),
}

impl BaseChoice {
    pub fn since(&self) -> Since {
        match self {
            BaseChoice::MergeBase => Since::MergeBase,
            BaseChoice::Head => Since::Head,
            BaseChoice::Commit(c) => Since::Commit(c.oid.as_str().to_owned()),
        }
    }

    fn title(&self) -> String {
        match self {
            BaseChoice::MergeBase => "Merge base".to_owned(),
            BaseChoice::Head => "HEAD".to_owned(),
            BaseChoice::Commit(c) => c.subject.clone(),
        }
    }

    /// What fuzzy search matches.
    fn haystack(&self) -> String {
        match self {
            BaseChoice::Commit(c) => format!("{} {} {}", c.subject, c.author, c.oid),
            other => format!("{} {}", other.title(), other.hint()),
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            BaseChoice::MergeBase => "Branch and uncommitted work since it left the default branch",
            BaseChoice::Head => "Uncommitted work only; a commit moves the base",
            BaseChoice::Commit(_) => "",
        }
    }
}

/// The picker's list.
pub struct BasePickerDelegate {
    choices: Vec<BaseChoice>,
    haystacks: Vec<String>,
    matches: Vec<usize>,
    query: String,
    selected: Option<usize>,
    current: Since,
    tab: WeakEntity<ReviewTab>,
    loading: bool,
    now_ms: i64,
    utc_offset_s: i64,
}

impl BasePickerDelegate {
    /// The rows listed, in order.
    pub fn matches(&self) -> Vec<&BaseChoice> {
        self.matches.iter().map(|&i| &self.choices[i]).collect()
    }

    /// Whether the commits are still being read.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The row Enter would choose.
    pub fn selected_choice(&self) -> Option<&BaseChoice> {
        self.choices.get(*self.matches.get(self.selected?)?)
    }

    fn rerank(&mut self) {
        let mut ranked = Ranker::text().rank(&self.query, &self.haystacks);
        // A query that starts an id (`1a2b3c4`) finds that commit first.
        let q = self.query.trim().to_ascii_lowercase();
        if q.len() >= 4 && q.bytes().all(|b| b.is_ascii_hexdigit()) {
            let exact: Vec<usize> = (0..self.choices.len())
                .filter(|&i| {
                    matches!(&self.choices[i], BaseChoice::Commit(c) if c.oid.as_str().starts_with(&q))
                })
                .collect();
            ranked.retain(|i| !exact.contains(i));
            ranked.splice(0..0, exact);
        }
        self.matches = ranked;
        if self.selected.is_none_or(|s| s >= self.matches.len()) {
            self.selected = (!self.matches.is_empty()).then_some(0);
        }
    }

    fn add_commits(&mut self, commits: Vec<CommitInfo>) {
        for c in commits {
            let choice = BaseChoice::Commit(c);
            self.haystacks.push(choice.haystack());
            self.choices.push(choice);
        }
        self.loading = false;
        self.rerank();
    }
}

/// The picker's list state.
pub type BasePicker = Entity<ListState<BasePickerDelegate>>;

/// The picker while it is open (tests read it).
#[derive(Default)]
struct OpenPicker(Option<WeakEntity<ListState<BasePickerDelegate>>>);

impl Global for OpenPicker {}

/// The open picker, if any.
pub fn current(cx: &App) -> Option<BasePicker> {
    cx.try_global::<OpenPicker>()?.0.as_ref()?.upgrade()
}

/// Opens the picker for `tab` (a live review; else does nothing), its
/// search field focused.
pub fn open(
    tab: &mut ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Option<BasePicker> {
    let current = super::since(tab)?;
    let worktree = super::live(tab)?.target().worktree.clone();
    let choices = vec![BaseChoice::MergeBase, BaseChoice::Head];
    let now = now_ms();
    let mut delegate = BasePickerDelegate {
        haystacks: choices.iter().map(BaseChoice::haystack).collect(),
        choices,
        matches: Vec::new(),
        query: String::new(),
        selected: None,
        current,
        tab: cx.entity().downgrade(),
        loading: true,
        now_ms: now,
        utc_offset_s: local_utc_offset_s(now),
    };
    delegate.rerank();
    let state = cx.new(|cx| {
        let mut state = ListState::new(delegate, window, cx).searchable(true);
        state.set_selected_index(Some(IndexPath::default()), window, cx);
        state
    });
    cx.set_global(OpenPicker(Some(state.downgrade())));
    let list = state.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(layout::PICKER_W))
            .margin_top(px(layout::PICKER_TOP))
            .close_button(false)
            .p_0()
            .child(
                // The picker's frame (ADR-0031): the dialog's whole content.
                div().debug_selector(|| "base-picker".into()).child(
                    List::new(&list)
                        .search_placeholder("Choose a base…")
                        .max_h(px(layout::OVERLAY_MAX_H))
                        .p(px(edge::OVERLAY)),
                ),
            )
    });
    state.update(cx, |s, cx| s.focus(window, cx));
    let weak = state.downgrade();
    cx.spawn_in(window, async move |_, cx| {
        let commits = cx
            .background_spawn(async move { list_commits(&Git::new(&worktree), 0, COMMITS) })
            .await;
        let commits = commits.unwrap_or_else(|e| {
            tracing::warn!("listing commits for the base picker: {e}");
            Vec::new()
        });
        let _ = weak.update(cx, |state, cx| {
            state.delegate_mut().add_commits(commits);
            cx.notify();
        });
    })
    .detach();
    Some(state)
}

/// Opens the live review of `tab`'s worktree with base `since` in its own
/// tab (focusing it when it is open already).
pub fn choose(
    tab: &Entity<ReviewTab>,
    since: Since,
    window: &mut Window,
    cx: &mut App,
) -> Option<Task<anyhow::Result<Entity<ReviewTab>>>> {
    let worktree = super::live(tab.read(cx))?.target().worktree.clone();
    let req = OpenRequest {
        worktree,
        source: Source::Live { since },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    Some(open_review(req, window, cx))
}

fn close(window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
    cx.set_global(OpenPicker(None));
}

impl ListDelegate for BasePickerDelegate {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_owned();
        self.selected = None;
        self.rerank();
        // See `tree::finder`: gpui-kit picks the selection from its row
        // cache, which still describes the previous query.
        cx.defer_in(window, |state, window, cx| {
            let first = (!state.delegate().matches.is_empty()).then(IndexPath::default);
            if state.selected_index() != first {
                state.set_selected_index(first, window, cx);
            }
        });
        cx.notify();
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
        let choice = self.choices.get(*self.matches.get(ix.row)?)?;
        let theme = cx.theme();
        let checked = choice.since() == self.current;
        let row = ix.row;
        let (title, detail, pill) = match choice {
            BaseChoice::Commit(c) => {
                let when = relative_time(self.now_ms, c.authored_at * 1000, self.utc_offset_s);
                (
                    c.subject.clone(),
                    format!("{} committed {when}", c.author),
                    Some(c.oid.short().to_owned()),
                )
            }
            other => (other.title(), other.hint().to_owned(), None),
        };
        Some(
            ListItem::new(("base-choice", ix.row))
                .debug_selector(move || format!("base-row-{row}"))
                .selected(self.selected == Some(ix.row))
                .h(px(height::ROW2))
                .px(px(pad::TEXT))
                .rounded(px(radius::for_height(height::ROW2)))
                .child(
                    h_flex()
                        .debug_selector(move || format!("base-choice-{row}"))
                        .w_full()
                        .gap(px(gap::ICON_LABEL))
                        .child(div().w(px(size::ICON_SM)).flex_none().when(checked, |d| {
                            d.child(
                                Icon::new(IconName::Check)
                                    .xsmall()
                                    .text_color(theme.primary),
                            )
                        }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_style(text::UI)
                                        .font_medium()
                                        .text_color(theme.foreground)
                                        .child(SharedString::from(title)),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_style(text::SMALL)
                                        .text_color(theme.muted_foreground)
                                        .child(SharedString::from(detail)),
                                ),
                        )
                        .children(pill.map(|p| sha_pill(&p, cx).into_any_element())),
                ),
        )
    }

    fn render_section_header(
        &mut self,
        _section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        Some(
            list_header(cx).child(
                div()
                    .debug_selector(|| "base-picker-header".into())
                    .child("Compare the working tree with…"),
            ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let text = if self.loading {
            "Reading the log…"
        } else {
            "No matching commits"
        };
        div()
            .py(px(gap::SECTION))
            .w_full()
            .flex()
            .justify_center()
            .text_style(text::UI)
            .text_color(cx.theme().muted_foreground)
            .child(text)
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix.map(|ix| ix.row);
        cx.notify();
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        let Some(since) = self.selected_choice().map(BaseChoice::since) else {
            return;
        };
        close(window, cx);
        let Some(tab) = self.tab.upgrade() else {
            return;
        };
        if since == self.current {
            return;
        }
        let task = choose(&tab, since, window, cx);
        if let Some(task) = task {
            task.detach();
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        close(window, cx);
    }
}
