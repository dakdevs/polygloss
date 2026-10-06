//! Step 1 of the open flow (design §11.3): the recent repos, fuzzy-ranked by
//! path with nucleo as you type, and "Browse…" (a native folder picker, run by
//! [`super::OpenFlow::browse`]).
//!
//! The list is a gpui-kit [`ListState`] with its search field. Section 0 holds
//! the matching repos, section 1 the "Browse…" row, which shows whatever the
//! query. Repos whose directory is gone are not offered. Choosing a row emits
//! `ListEvent::Confirm`, which the flow turns into
//! [`super::OpenFlow::choose_repo`] or [`super::OpenFlow::browse`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gpui_kit::component::list::{ListDelegate, ListItem, ListState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, Task, Window, div, px,
};
use polygloss_core::review::RecentRepo;

use crate::open_flow::ranking::Ranker;
use crate::space::{TextStyleExt as _, gap, height, pad, radius, stroke, text};

/// How many recent repos the list offers.
pub const RECENT_REPOS: u32 = 50;

/// One recent repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    /// Where to open it: the main worktree (or the bare repo).
    pub path: PathBuf,
    /// The repo's name (its directory).
    pub name: SharedString,
    /// Where it is, with the home directory as `~`.
    pub detail: SharedString,
}

impl RepoRow {
    pub fn new(repo: &RecentRepo) -> RepoRow {
        RepoRow {
            path: repo.path.clone(),
            name: repo.display_name.clone().into(),
            detail: tildify(&repo.path).into(),
        }
    }
}

/// What the user chose in the repo list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoChoice {
    Repo(PathBuf),
    Browse,
}

/// The repo list's delegate.
pub struct RepoDelegate {
    rows: Vec<RepoRow>,
    /// What the ranker matches: each row's path as shown (`~/src/app`).
    haystacks: Vec<String>,
    /// Indices into `rows`, best first.
    matches: Vec<usize>,
    query: String,
    ranker: Ranker,
    loaded: bool,
}

impl Default for RepoDelegate {
    fn default() -> Self {
        RepoDelegate {
            rows: Vec::new(),
            haystacks: Vec::new(),
            matches: Vec::new(),
            query: String::new(),
            ranker: Ranker::paths(),
            loaded: false,
        }
    }
}

impl RepoDelegate {
    /// Replaces the rows (most recently opened first) and re-applies the
    /// query.
    pub fn set_rows(&mut self, rows: Vec<RepoRow>) {
        self.haystacks = rows.iter().map(|r| r.detail.to_string()).collect();
        self.rows = rows;
        self.loaded = true;
        self.rerank();
    }

    /// Whether the recent repos have been read.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// The repos matching the query, best first.
    pub fn matches(&self) -> impl Iterator<Item = &RepoRow> {
        self.matches.iter().map(|&ix| &self.rows[ix])
    }

    /// The list always ends with "Browse…".
    pub fn has_browse_row(&self) -> bool {
        true
    }

    /// What row `ix` stands for.
    pub fn choice(&self, ix: IndexPath) -> Option<RepoChoice> {
        match ix.section {
            0 => self
                .matches
                .get(ix.row)
                .map(|&i| RepoChoice::Repo(self.rows[i].path.clone())),
            1 if ix.row == 0 => Some(RepoChoice::Browse),
            _ => None,
        }
    }

    fn rerank(&mut self) {
        self.matches = self.ranker.rank(&self.query, &self.haystacks);
    }
}

impl ListDelegate for RepoDelegate {
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

    fn sections_count(&self, _cx: &App) -> usize {
        2
    }

    fn items_count(&self, section: usize, _cx: &App) -> usize {
        match section {
            0 => self.matches.len(),
            _ => 1,
        }
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let theme = cx.theme();
        let (icon, title, detail, selector) = match self.choice(ix)? {
            RepoChoice::Repo(_) => {
                let row = &self.rows[self.matches[ix.row]];
                (
                    IconName::Folder,
                    row.name.clone(),
                    row.detail.clone(),
                    format!("open-flow-repo-{}", ix.row),
                )
            }
            RepoChoice::Browse => (
                IconName::FolderOpen,
                SharedString::from("Browse…"),
                SharedString::from("Choose a repository folder on disk"),
                "open-flow-browse".to_owned(),
            ),
        };
        Some(
            ListItem::new(("open-flow-repo", ix.section * 100_000 + ix.row))
                .when(ix.section == 0, |item| {
                    item.debug_selector(move || format!("open-flow-repo-row-{}", ix.row))
                })
                // Every row is as tall (the list measures one for all).
                .h(px(height::ROW2))
                .px(px(pad::TEXT))
                .rounded(px(radius::for_height(height::ROW2)))
                .child(
                    h_flex()
                        .debug_selector(move || selector.clone())
                        .gap(px(gap::ICON_LABEL))
                        .min_w_0()
                        .child(Icon::new(icon).small().text_color(theme.muted_foreground))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_style(text::UI)
                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_style(text::SMALL)
                                        .text_color(theme.muted_foreground)
                                        .child(detail),
                                ),
                        ),
                ),
        )
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        // Every header gets the height of section 0's (the list measures
        // one): the "Browse…" section's is a divider in the same frame.
        let border = cx.theme().border;
        let header = list_header(cx);
        Some(match section {
            0 => header.child(
                div()
                    .debug_selector(|| "open-flow-repo-header".into())
                    .child("RECENT REPOSITORIES"),
            ),
            _ => header.child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().h(px(stroke::BORDER)).bg(border))
                    .child(div().w_0().invisible().child("·")),
            ),
        })
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div()
            .py(px(gap::SECTION))
            .text_style(text::UI)
            .text_color(cx.theme().muted_foreground)
            .child("No matching repositories")
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }
}

/// An overlay list's section header (ADR-0031: one header for every
/// overlay): its text on the rows' content column (`OVERLAY` + `TEXT`, as
/// the rows' `TEXT` inside a list padded by `OVERLAY`), `GROUP` above it and
/// `INLINE` below.
pub fn list_header(cx: &App) -> Div {
    div()
        .px(px(pad::TEXT))
        .pt(px(gap::GROUP))
        .pb(px(gap::INLINE))
        .text_style(text::SMALL)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
}

/// Reads the recent repos whose directory still exists (on the background
/// executor: it touches the disk).
pub fn load_rows(repos: Vec<RecentRepo>) -> Vec<RepoRow> {
    repos
        .iter()
        .filter(|r| r.path.is_dir())
        .map(RepoRow::new)
        .collect()
}

/// `path` with the home directory as `~` (`HOME` as set, or its real path).
pub fn tildify(path: &Path) -> String {
    static HOMES: OnceLock<Vec<PathBuf>> = OnceLock::new();
    let homes = HOMES.get_or_init(|| {
        let Some(home) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|h| !h.as_os_str().is_empty())
        else {
            return Vec::new();
        };
        let real = std::fs::canonicalize(&home).ok().filter(|r| *r != home);
        std::iter::once(home).chain(real).collect()
    });
    for home in homes {
        if let Ok(rest) = path.strip_prefix(home) {
            return if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}
