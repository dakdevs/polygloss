//! The Files segment's accordion (design §11.5, OQ-44): **Changes** (the
//! uncategorized files), then one panel per non-empty enabled category, in
//! category order, built from the tab's [`Partition`]. When every file is
//! categorized, Changes is left out.
//!
//! - **One open at a time:** each panel is a [`PanelTree`] with its own
//!   gpui-kit `TreeState` (virtualized, scrolling on its own) and its own
//!   collapsed folders; [`FilesPanel`] holds them and which one is open.
//!   Only the open panel's tree is rendered; the others are their headers.
//!   Switching is instant (no gpui-component `Accordion`: no motion).
//! - **Headers:** chevron, the category's icon, its title and file count
//!   ("3 of 12" while the filter is on), then, over the panel's files, the
//!   changed-since-viewed dot, the open-thread count and the agent badge. A
//!   click opens the panel and gives its list the keyboard; the list keeps
//!   its scroll position. A lone Changes panel (nothing categorized) shows
//!   no header: it is the plain tree of the reference.
//! - **Filter:** the segment's one [`TreeFilter`] applies to every panel;
//!   each reports `(matches, total)`; a panel without a match is hidden.
//! - **Folders** cover their own panel's files only (a `src/` in Changes
//!   and one in Tests are two rows with two Viewed slots).
//!
//! Debug selectors (`key` = [`PanelKey`]'s text: `changes`, `tests`,
//! `custom:tokens`): `tree-panel-{key}` (the header), `tree-panel-title-{key}:
//! {title}`, `tree-panel-count-{key}: {count}`, `tree-panel-threads-{key}:
//! {n}`, `tree-panel-agent-{key}`, `tree-panel-changed-{key}`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, div, px,
};
use polygloss_core::categories::CategoryId;
use polygloss_diff::FileChange;
use polygloss_viewport::{BandFlags, FileFlags, group_digits};

use super::filters::TreeFilter;
use super::model::{NodeId, TreeModel};
use super::row::Check;
use super::{FileTree, row};
use crate::categories::Partition;
use crate::review_tab::toolbar::tooltip;

/// A panel header's height (pt).
const HEADER_HEIGHT: f32 = 30.0;
/// The Changes panel's icon (a category's is its own).
const CHANGES_ICON: &str = "icons/file-text.svg";

/// Which panel: Changes, or a category's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PanelKey {
    Changes,
    Category(CategoryId),
}

impl fmt::Display for PanelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PanelKey::Changes => f.write_str("changes"),
            PanelKey::Category(id) => write!(f, "{id}"),
        }
    }
}

impl PanelKey {
    /// How view state names this panel's folder `path`: Changes' plainly
    /// (as before panels existed), a category's as `<category>:<path>`.
    fn dir_key(&self, path: &str) -> String {
        match self {
            PanelKey::Changes => path.to_owned(),
            PanelKey::Category(id) => format!("{id}:{path}"),
        }
    }
}

/// One panel: its files' tree and its own list state.
pub struct PanelTree {
    key: PanelKey,
    title: SharedString,
    /// `icons/<name>.svg`.
    icon: SharedString,
    /// Its files, in git order (which is display order within a panel).
    files: Vec<u32>,
    full: Arc<TreeModel>,
    /// The tree of the files the filter keeps, while it is on.
    filtered: Option<Arc<TreeModel>>,
    state: Entity<TreeState>,
    /// Viewed and all files per directory of the tree shown.
    dir_viewed: Arc<HashMap<String, (u32, u32)>>,
    /// Over all its files.
    flags: BandFlags,
    /// Folders collapsed in the unfiltered tree (all start expanded).
    collapsed: HashSet<String>,
    /// Folders collapsed while filtering (reset by each filter change).
    filter_collapsed: HashSet<String>,
    _subscriptions: [Subscription; 2],
}

/// What a panel shows before it is built: key, title, icon, files.
pub(super) struct PanelSpec {
    pub key: PanelKey,
    pub title: SharedString,
    pub icon: SharedString,
    pub files: Vec<u32>,
}

impl PanelSpec {
    /// The panels of `partition` (every file in Changes without one): Changes
    /// unless it is empty while categories hold files, then each section.
    /// No panel without files.
    pub fn of(partition: Option<&Partition>, files: usize) -> Vec<PanelSpec> {
        let changes = |files: Vec<u32>| PanelSpec {
            key: PanelKey::Changes,
            title: "Changes".into(),
            icon: CHANGES_ICON.into(),
            files,
        };
        let Some(p) = partition else {
            return (files > 0)
                .then(|| changes((0..files as u32).collect()))
                .into_iter()
                .collect();
        };
        let mut out = Vec::new();
        if !p.main.is_empty() {
            out.push(changes(p.main.clone()));
        }
        out.extend(p.sections.iter().map(|s| PanelSpec {
            key: PanelKey::Category(s.id.clone()),
            title: s.info.title.clone().into(),
            icon: format!("icons/{}.svg", s.info.icon).into(),
            files: s.files.clone(),
        }));
        out
    }
}

impl PanelTree {
    /// A new panel for `spec`, its list state reporting to `cx`'s tree.
    pub(super) fn new(
        spec: PanelSpec,
        all: &[FileChange],
        cx: &mut Context<FileTree>,
    ) -> PanelTree {
        let state = cx.new(|cx| TreeState::new(cx));
        let key = spec.key.clone();
        let observe = cx.observe(&state, move |t: &mut FileTree, _, cx| {
            t.selection_changed(&key, cx)
        });
        let key = spec.key.clone();
        let expand = cx.subscribe(&state, move |t: &mut FileTree, _, event: &TreeEvent, cx| {
            t.expansion_changed(&key, event, cx)
        });
        let mut panel = PanelTree {
            key: spec.key.clone(),
            title: SharedString::default(),
            icon: SharedString::default(),
            files: Vec::new(),
            full: Arc::default(),
            filtered: None,
            state,
            dir_viewed: Arc::default(),
            flags: BandFlags::default(),
            collapsed: HashSet::new(),
            filter_collapsed: HashSet::new(),
            _subscriptions: [observe, expand],
        };
        panel.reset(spec, all);
        panel
    }

    /// Takes `spec`'s files (a new partition), keeping the list state and
    /// the collapsed folders that still exist.
    pub(super) fn reset(&mut self, spec: PanelSpec, all: &[FileChange]) {
        self.title = spec.title;
        self.icon = spec.icon;
        self.full = Arc::new(build(&spec.files, all));
        // The tree walks files depth-first, the viewport in display order
        // (git order within a panel); `n`/`p` rely on them agreeing, which
        // holds while git lists each directory's files together
        // (`tree_order_matches_diff_order`).
        debug_assert!(
            self.full.file_order().windows(2).all(|w| w[0] < w[1]),
            "tree order differs from diff order"
        );
        self.files = spec.files;
        let dirs: HashSet<String> = self.full.dir_paths().into_iter().collect();
        self.collapsed.retain(|d| dirs.contains(d));
        self.filtered = None;
        self.filter_collapsed.clear();
    }

    pub fn key(&self) -> &PanelKey {
        &self.key
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Its files, in git order.
    pub fn files(&self) -> &[u32] {
        &self.files
    }

    /// The tree shown (filtered or not).
    pub fn model(&self) -> &TreeModel {
        self.filtered.as_deref().unwrap_or(&self.full)
    }

    pub(super) fn shared_model(&self) -> Arc<TreeModel> {
        self.filtered.clone().unwrap_or_else(|| self.full.clone())
    }

    /// Its gpui-kit list state (selection, focus, scroll).
    pub fn tree_state(&self) -> &Entity<TreeState> {
        &self.state
    }

    /// `(files the filter keeps, all its files)`.
    pub fn matches(&self) -> (u32, u32) {
        (
            self.model().file_order().len() as u32,
            self.files.len() as u32,
        )
    }

    /// Whether it shows: it has files the filter keeps.
    pub fn is_shown(&self) -> bool {
        self.matches().0 > 0
    }

    /// The review indicators over its files.
    pub fn flags(&self) -> BandFlags {
        self.flags
    }

    /// Folder `dir`'s tri-state Viewed slot in this panel's tree shown.
    pub fn folder_check(&self, dir: &str) -> Option<Check> {
        self.dir_viewed
            .get(dir)
            .map(|&(viewed, total)| Check::of(viewed, total))
    }

    pub(super) fn dir_viewed(&self) -> Arc<HashMap<String, (u32, u32)>> {
        self.dir_viewed.clone()
    }

    /// The folders collapsed in the tree shown.
    pub(super) fn collapsed_mut(&mut self) -> &mut HashSet<String> {
        if self.filtered.is_some() {
            &mut self.filter_collapsed
        } else {
            &mut self.collapsed
        }
    }

    pub(super) fn all_expanded(&self) -> bool {
        self.collapsed.is_empty()
    }

    /// Its expanded folders of the unfiltered tree, as view state names
    /// them ([`PanelKey::dir_key`]), in tree order.
    pub(super) fn expanded_dirs(&self) -> impl Iterator<Item = String> + '_ {
        self.full
            .dir_paths()
            .into_iter()
            .filter(|d| !self.collapsed.contains(d))
            .map(|d| self.key.dir_key(&d))
    }

    /// Expands exactly the folders of `expanded` (view state's names).
    pub(super) fn set_expanded(&mut self, expanded: &HashSet<String>) {
        let key = self.key.clone();
        self.collapsed = self
            .full
            .dir_paths()
            .into_iter()
            .filter(|d| !expanded.contains(&key.dir_key(d)))
            .collect();
    }

    /// Applies `filter` to its files.
    pub(super) fn filter(&mut self, filter: &TreeFilter, all: &[FileChange], flags: &[FileFlags]) {
        self.filtered = filter
            .is_active()
            .then(|| Arc::new(build(&filter.apply(all, flags, &self.files), all)));
    }

    /// Counts its folders' Viewed files and its indicators from `flags`.
    pub(super) fn recount(&mut self, flags: &[FileFlags]) {
        let flag = |f: u32| flags.get(f as usize).copied().unwrap_or_default();
        self.dir_viewed = Arc::new(
            self.model()
                .nodes()
                .iter()
                .filter(|n| n.is_dir())
                .map(|n| {
                    let viewed = n.files.iter().filter(|&&f| flag(f).viewed).count() as u32;
                    (n.path.clone(), (viewed, n.files.len() as u32))
                })
                .collect(),
        );
        self.flags = self.files.iter().fold(BandFlags::default(), |mut b, &f| {
            let f = flag(f);
            b.open_threads += f.open_threads;
            b.agent |= f.agent_threads;
            b.changed_since_viewed |= f.changed_since_viewed;
            b
        });
    }

    /// Gives its list state the tree shown, folders expanded unless
    /// collapsed.
    pub(super) fn push_items(&self, cx: &mut App) {
        let collapsed = if self.filtered.is_some() {
            &self.filter_collapsed
        } else {
            &self.collapsed
        };
        let items = tree_items(self.model(), collapsed);
        self.state.update(cx, |s, cx| s.set_items(items, cx));
    }
}

/// The tree of `files` (indices into `all`).
fn build(files: &[u32], all: &[FileChange]) -> TreeModel {
    TreeModel::build(
        files
            .iter()
            .filter_map(|&f| Some((f, all.get(f as usize)?.display_path()))),
    )
}

/// gpui-kit tree items for `model`, directories expanded unless in
/// `collapsed`.
fn tree_items(model: &TreeModel, collapsed: &HashSet<String>) -> Vec<TreeItem> {
    fn build_item(model: &TreeModel, id: NodeId, collapsed: &HashSet<String>) -> TreeItem {
        let node = model.node(id);
        let item = TreeItem::new(node.item_id(), node.name.clone());
        if node.is_dir() {
            item.expanded(!collapsed.contains(&node.path)).children(
                node.children
                    .iter()
                    .map(|&c| build_item(model, c, collapsed)),
            )
        } else {
            item
        }
    }
    model
        .roots()
        .iter()
        .map(|&r| build_item(model, r, collapsed))
        .collect()
}

/// The panels of a tree and the open one.
#[derive(Default)]
pub struct FilesPanel {
    panels: Vec<PanelTree>,
    open: Option<usize>,
    /// Each file's panel, by `file_idx` (`u32::MAX`: none).
    owner: Vec<u32>,
}

impl FilesPanel {
    /// Every panel, top to bottom (hidden ones included).
    pub fn panels(&self) -> &[PanelTree] {
        &self.panels
    }

    pub fn open(&self) -> Option<&PanelTree> {
        self.panels.get(self.open?)
    }

    pub fn open_key(&self) -> Option<&PanelKey> {
        self.open().map(PanelTree::key)
    }

    pub(super) fn panels_mut(&mut self) -> &mut [PanelTree] {
        &mut self.panels
    }

    pub(super) fn get_mut(&mut self, key: &PanelKey) -> Option<&mut PanelTree> {
        self.panels.iter_mut().find(|p| p.key == *key)
    }

    pub(super) fn index_of(&self, key: &PanelKey) -> Option<usize> {
        self.panels.iter().position(|p| p.key == *key)
    }

    /// The panel holding file `file_idx`.
    pub(super) fn panel_of(&self, file_idx: u32) -> Option<usize> {
        let i = *self.owner.get(file_idx as usize)?;
        (i != u32::MAX).then_some(i as usize)
    }

    /// Opens panel `ix`; `false` when it does not show.
    pub(super) fn set_open(&mut self, ix: usize) -> bool {
        let shown = self.panels.get(ix).is_some_and(PanelTree::is_shown);
        if shown {
            self.open = Some(ix);
        }
        shown
    }

    /// The open panel when it shows, else the first one that does.
    pub(super) fn ensure_open_shown(&mut self) {
        if self.open().is_some_and(PanelTree::is_shown) {
            return;
        }
        self.open = self
            .panels
            .iter()
            .position(PanelTree::is_shown)
            .or(self.open);
    }

    /// Replaces the panels by `panels` (each file's owner recomputed for
    /// `files` files), none open.
    pub(super) fn replace(&mut self, panels: Vec<PanelTree>, files: usize) {
        let mut owner = vec![u32::MAX; files];
        for (i, p) in panels.iter().enumerate() {
            for &f in &p.files {
                if let Some(o) = owner.get_mut(f as usize) {
                    *o = i as u32;
                }
            }
        }
        self.owner = owner;
        self.panels = panels;
        self.open = None;
    }

    /// Takes every panel out (to reuse their list states).
    pub(super) fn take(&mut self) -> Vec<PanelTree> {
        self.open = None;
        std::mem::take(&mut self.panels)
    }

    /// Whether headers show: more than one panel, or a category's alone.
    pub(super) fn has_headers(&self) -> bool {
        self.panels.len() > 1 || self.panels.iter().any(|p| p.key != PanelKey::Changes)
    }
}

/// A panel's header: a click opens it.
pub(super) fn header(
    panel: &PanelTree,
    open: bool,
    filtering: bool,
    tree: WeakEntity<FileTree>,
    first: bool,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let key = panel.key.to_string();
    let (matches, total) = panel.matches();
    let count = if filtering {
        format!(
            "{} of {}",
            group_digits(matches.into()),
            group_digits(total.into())
        )
    } else {
        group_digits(total.into())
    };
    let flags = panel.flags;
    let selector = format!("tree-panel-{key}");
    let open_key = panel.key.clone();
    let named = |part: &str, text: Option<String>| {
        let selector = match &text {
            Some(text) => format!("tree-panel-{part}-{key}: {text}"),
            None => format!("tree-panel-{part}-{key}"),
        };
        div().debug_selector(move || selector)
    };
    h_flex()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .flex_none()
        .h(px(HEADER_HEIGHT))
        .px_2()
        .gap_1p5()
        .text_sm()
        .cursor_pointer()
        .when(!first, |el| el.border_t_1().border_color(theme.border))
        .hover(|s| s.bg(theme.foreground.opacity(0.04)))
        .on_click(move |_, window, cx| {
            if let Some(tree) = tree.upgrade() {
                tree.update(cx, |t, cx| {
                    t.open_panel(&open_key, cx);
                    t.focus(window, cx);
                });
            }
        })
        .child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(theme.muted_foreground),
        )
        .child(
            Icon::empty()
                .path(panel.icon.clone())
                .small()
                .text_color(theme.muted_foreground),
        )
        .child(
            named("title", Some(panel.title.to_string()))
                .font_medium()
                .truncate()
                .child(panel.title.clone()),
        )
        .child(
            named("count", Some(count.clone()))
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(count),
        )
        .child(div().flex_1())
        .when(flags.changed_since_viewed, |el| {
            el.child(
                named("changed", None)
                    .id(SharedString::from(format!(
                        "tree-panel-changed-{}",
                        panel.key
                    )))
                    .size(px(6.))
                    .rounded_full()
                    .bg(theme.blue)
                    .tooltip(tooltip("Changed since viewed")),
            )
        })
        .when(flags.open_threads > 0, |el| {
            let n = flags.open_threads;
            el.child(
                named("threads", Some(n.to_string()))
                    .id(SharedString::from(format!(
                        "tree-panel-threads-{}",
                        panel.key
                    )))
                    .flex_none()
                    .px_1p5()
                    .rounded_full()
                    .bg(theme.muted)
                    .text_xs()
                    .child(n.to_string())
                    .tooltip(tooltip(match n {
                        1 => "1 open thread".to_owned(),
                        n => format!("{n} open threads"),
                    })),
            )
        })
        .when(flags.agent, |el| {
            el.child(
                named("agent", None)
                    .text_color(theme.primary)
                    .child(Icon::new(IconName::Bot).xsmall()),
            )
        })
}

impl FileTree {
    fn row_ctx(&self, panel: &PanelTree, cx: &Context<Self>) -> row::RowCtx {
        let colors = crate::theme::viewport_theme(cx);
        row::RowCtx {
            files: self.files.clone(),
            flags: self.flags.clone(),
            dir_viewed: panel.dir_viewed(),
            model: panel.shared_model(),
            viewport: self.viewport.clone(),
            tree: cx.entity().downgrade(),
            status: row::StatusColors::of(cx),
            stat_added: colors.stat_added,
            stat_removed: colors.stat_removed,
        }
    }

    /// The open panel's tree, or why nothing shows.
    pub(super) fn body(&self, cx: &Context<Self>) -> AnyElement {
        let Some(panel) = self.panels.open().filter(|p| p.is_shown()) else {
            let theme = cx.theme();
            let none = self.files.is_empty();
            let this = cx.entity().downgrade();
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if none {
                    "No files changed"
                } else {
                    "No files match the filters"
                })
                .when(!none, |el| {
                    el.child(
                        Button::new("tree-clear-filters")
                            .debug_selector(|| "tree-clear-filters".into())
                            .label("Clear filters")
                            .xsmall()
                            .outline()
                            .on_click(move |_, window, cx| {
                                if let Some(t) = this.upgrade() {
                                    t.update(cx, |t, cx| t.clear_filters(window, cx));
                                }
                            }),
                    )
                })
                .into_any_element();
        };
        let ctx = row::RowCtx::shared(self.row_ctx(panel, cx));
        div()
            .flex_1()
            .min_h_0()
            .px_1p5()
            .child(
                tree(
                    panel.tree_state(),
                    move |ix, entry, selected, window, cx| {
                        row::render(&ctx, ix, entry, selected, window, cx)
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}
