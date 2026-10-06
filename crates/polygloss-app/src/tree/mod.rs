//! File tree and file finder (⌘P) (design §11.5, §11.8).
//!
//! - [`FileTree`]: the sidebar's Files segment: the rounded filter field
//!   holding the filter menu, the accordion of [`panels`] (Changes, then one
//!   per non-empty category; each a gpui-kit `Tree`, virtualized, key
//!   context `Tree`, over its files with directory chains compacted,
//!   [`model`]), and the [`footer`] totals and chips. Rows ([`row`]) carry
//!   the outline icon, open-thread and agent badges, the "changed since
//!   viewed" dot, `+a −d`, the status letter and the Viewed slot at their
//!   end; the one [`filters::TreeFilter`] hides files in every panel
//!   (unviewed, has comments, status, extension, fuzzy text). Selecting a
//!   file scrolls the viewport to it (opening its section); the viewport's
//!   top file is highlighted in the tree.
//! - **Panels follow the viewport** (OQ-44): when its top file moves into
//!   another panel's files (a jump, a scroll across a section's edge, the
//!   first partition or one moving the top file), that panel opens, and
//!   takes the keyboard from the list that had it; a panel the user opens
//!   stays open until the next such crossing. The panels are rebuilt from
//!   the tab's partition on [`Repartitioned`] (once per partition: attach,
//!   a settings reload, a palette toggle, a refresh or an iteration
//!   switch).
//! - The pane is a cached view: the diff's scroll frames do not render it.
//!   It renders again when notified: its own changes, new line counts
//!   (`ViewportEvent::CountsUpdated`, `BinaryDetected`) and a new top file.
//! - [`finder`]: ⌘P, every changed path ranked by `nucleo-matcher`; Enter
//!   jumps to the file.
//!
//! The filter (`tree::FocusFilter`) and ⌘P show the sidebar's Files segment
//! first ([`crate::chrome::show_files`]).
//!
//! The Viewed state is T3.7's: it pushes [`FileFlags`] with
//! [`FileTree::set_file_flags`] and handles the slots' [`FileTreeEvent`]s
//! and `tree::ToggleViewed` (with [`FileTree::selected_file`] /
//! [`FileTree::selected_dir`], of the open panel). View-state (T3.14) reads
//! and restores every panel's expansion with [`FileTree::expanded_dirs`] /
//! [`FileTree::set_expanded_dirs`].

pub mod filters;
pub mod finder;
pub mod footer;
pub mod model;
pub mod panels;
pub mod row;

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::tree::{TreeEvent, TreeState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, MenuItem, ParentElement as _, Render,
    ScrollStrategy, SharedString, StyleRefinement, Styled as _, Subscription, WeakEntity, Window,
    div, px,
};
use polygloss_diff::FileChange;
use polygloss_viewport::{DiffViewport, FileFlags, ScrollTarget, ViewportEvent};

use crate::categories::{Partition, Repartitioned};
use crate::keyboard::menu::KeyMenu;
use crate::keymap::actions::{tree as tree_actions, window as window_actions};
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::window::MenuKind;
use filters::{StatusFilter, TreeFilter};
use model::{ItemId, TreeModel};
use panels::{FilesPanel, PanelKey, PanelSpec, PanelTree};

/// Registers the tree's and the finder's actions and menu items.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tree_actions::NextFile, _, cx| {
            if let Some(tree) = file_tree(tab) {
                tree.update(cx, |t, cx| t.next_file(cx));
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tree_actions::PrevFile, _, cx| {
            if let Some(tree) = file_tree(tab) {
                tree.update(cx, |t, cx| t.prev_file(cx));
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tree_actions::FocusFilter, window, cx| {
            crate::chrome::show_files(window, cx);
            if let Some(tree) = file_tree(tab) {
                tree.update(cx, |t, cx| t.focus_filter(window, cx));
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tree_actions::FilterMenu, window, cx| {
            if let Some(tree) = file_tree(tab) {
                tree.update(cx, |t, cx| t.open_filter_menu(window, cx));
            }
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &window_actions::FileFinder, window, cx| {
            crate::chrome::show_files(window, cx);
            finder::open(tab, window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::View,
        vec![MenuItem::action("Go to File…", window_actions::FileFinder)],
        cx,
    );
}

/// The tab's tree (a [`ReviewTab`] extension).
struct TreePane(Entity<FileTree>);

/// The file tree of `tab`, once attached.
pub fn file_tree(tab: &ReviewTab) -> Option<&Entity<FileTree>> {
    tab.extension::<TreePane>().map(|p| &p.0)
}

/// Creates the tab's [`FileTree`], empty: its panels come with the tab's
/// first partition ([`Repartitioned`], before the first frame), and again
/// with every later one.
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let viewport = tab.viewport.clone();
    let tree = cx.new(|cx| FileTree::new(Arc::default(), viewport, window, cx));
    tab.insert_extension(TreePane(tree));
    cx.subscribe_self(|tab: &mut ReviewTab, _: &Repartitioned, cx| {
        let Some(tree) = file_tree(tab).cloned() else {
            return;
        };
        let files = tab.opened.files.clone();
        let partition = crate::categories::partition(tab);
        let flags = tab.viewport.read(cx).file_flags().to_vec();
        tree.update(cx, |t, cx| t.set_partition(files, partition, flags, cx));
    })
    .detach();
}

/// The file tree pane, cached: the tab renders on every scroll frame of the
/// diff, the tree only when it is notified.
pub fn render_pane(
    tab: &ReviewTab,
    _window: &mut Window,
    _cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    file_tree(tab).map(|t| {
        t.clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element()
    })
}

/// What the tree asks its host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTreeEvent {
    /// A file's Viewed slot was clicked.
    ToggleViewed(u32),
    /// A directory's Viewed slot was clicked: every file below it (as its
    /// panel's tree shows them).
    ToggleFolderViewed { dir: String, files: Vec<u32> },
}

/// One visible row of the open panel's tree (tests and the view-state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub id: ItemId,
    pub label: String,
    pub depth: usize,
    pub expanded: bool,
}

/// The file tree of one review tab.
pub struct FileTree {
    files: Arc<Vec<FileChange>>,
    viewport: Entity<DiffViewport>,
    filter_input: Entity<InputState>,
    /// The accordion: every panel and the open one.
    panels: FilesPanel,
    /// The partition the panels were built from (`None`: every file in
    /// Changes).
    partition: Option<Arc<Partition>>,
    flags: Arc<Vec<FileFlags>>,
    filter: TreeFilter,
    /// The open panel's selection as last seen, to tell the user's changes
    /// from ours.
    selected: Option<ItemId>,
    /// The file the tree marks as current.
    current: Option<u32>,
    /// The file last jumped to from the tree, while the viewport shows it.
    jumped: Option<u32>,
    /// The panel of the viewport's top file, as last seen (OQ-44: a new one
    /// is a crossing, which opens it).
    followed: Option<PanelKey>,
    /// An expansion restored before the panels were first built (view state
    /// restores before the tab's first partition reaches the tree).
    restored: Option<HashSet<String>>,
    /// The open panel as last rendered: when another one opens, the
    /// keyboard in the old list moves to the new one.
    rendered_open: Option<PanelKey>,
    /// The filter menu, when opened from the keyboard (`f`).
    key_menu: Option<KeyMenu>,
    /// Times [`Render::render`] ran ([`FileTree::render_count`]).
    renders: u64,
    /// Partitions applied ([`FileTree::panel_builds`]).
    builds: u64,
    _subscriptions: Vec<Subscription>,
}

/// The file tree's own key context, around its lists, filter box and
/// filter menu (no bindings use it: [`FileTree::contains_focus`] does).
pub const KEY_CONTEXT: &str = "FileTree";

impl EventEmitter<FileTreeEvent> for FileTree {}

/// The tree shown when no panel is open (no files).
static EMPTY: LazyLock<TreeModel> = LazyLock::new(TreeModel::default);

impl FileTree {
    /// The tree of `files`, all in one Changes panel, scrolling `viewport`
    /// ([`FileTree::set_partition`] splits them).
    pub fn new(
        files: Arc<Vec<FileChange>>,
        viewport: Entity<DiffViewport>,
        window: &mut Window,
        cx: &mut Context<FileTree>,
    ) -> FileTree {
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let subscriptions = vec![
            cx.subscribe(
                &viewport,
                |t: &mut FileTree, _, event: &ViewportEvent, cx| match *event {
                    ViewportEvent::VisibleFileChanged(idx) => t.visible_file_changed(idx, cx),
                    // Rows and the footer show the counts.
                    ViewportEvent::CountsUpdated | ViewportEvent::BinaryDetected(_) => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe(
                &filter_input,
                |t: &mut FileTree, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let query = input.read(cx).value().to_string();
                        t.set_query_text(query, cx);
                    }
                },
            ),
        ];
        let mut tree = FileTree {
            files: Arc::default(),
            viewport,
            filter_input,
            panels: FilesPanel::default(),
            partition: None,
            flags: Arc::default(),
            filter: TreeFilter::default(),
            selected: None,
            current: None,
            jumped: None,
            followed: None,
            restored: None,
            rendered_open: None,
            key_menu: None,
            renders: 0,
            builds: 0,
            _subscriptions: subscriptions,
        };
        tree.build(files, None, Vec::new(), cx);
        tree
    }

    /// How many times the tree has rendered: tests check that scroll
    /// frames of the diff do not render it.
    pub fn render_count(&self) -> u64 {
        self.renders
    }

    /// How many partitions rebuilt the panels.
    pub fn panel_builds(&self) -> u64 {
        self.builds
    }

    /// The accordion's panels.
    pub fn panels(&self) -> &FilesPanel {
        &self.panels
    }

    /// The open panel's tree shown (filtered or not).
    pub fn model(&self) -> &TreeModel {
        self.panels.open().map_or(&EMPTY, PanelTree::model)
    }

    /// The open panel's gpui-kit tree state (selection, focus, scroll).
    pub fn tree_state(&self) -> Option<&Entity<TreeState>> {
        self.panels.open().map(PanelTree::tree_state)
    }

    /// The fuzzy filter box.
    pub fn filter_input(&self) -> &Entity<InputState> {
        &self.filter_input
    }

    /// The filter box's focus handle.
    pub fn filter_focus(&self, cx: &App) -> FocusHandle {
        self.filter_input.focus_handle(cx)
    }

    /// Whether the filter menu opened from the keyboard (`f`) is open (the
    /// button's own dropdown, opened with the mouse, is gpui-kit's).
    pub fn filter_menu_open(&self) -> bool {
        self.key_menu.is_some()
    }

    /// `/`: the keyboard to the filter box, its text selected.
    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
    }

    /// `f`: the filter menu, opened from the keyboard under its button.
    pub fn open_filter_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tree = cx.entity().downgrade();
        // Counted when the menu opens, not on every render.
        let (f, extensions) = (self.filter.clone(), filters::extensions(&self.files));
        let menu = KeyMenu::open(
            |t: &mut FileTree| Some(&mut t.key_menu),
            self.key_menu.take(),
            move |menu, _, _| filters::menu(&tree, f, extensions, menu),
            window,
            cx,
        );
        self.key_menu = Some(menu);
        cx.notify();
    }

    /// The open panel's visible rows, top to bottom.
    pub fn rows(&self, cx: &App) -> Vec<TreeRow> {
        let Some(state) = self.tree_state() else {
            return Vec::new();
        };
        let state = state.read(cx);
        (0..)
            .map_while(|ix| state.entry(ix))
            .filter_map(|e| {
                Some(TreeRow {
                    id: ItemId::parse(&e.item().id)?,
                    label: e.item().label.to_string(),
                    depth: e.depth(),
                    expanded: e.is_expanded(),
                })
            })
            .collect()
    }

    /// Shows `files` split by `partition` (one panel per non-empty
    /// category after Changes) with their review state `flags`: the same
    /// filter, each panel's collapsed folders (by path), the open panel
    /// while it still shows (else the viewport's top file's) and a selected
    /// folder that still exists there.
    pub fn set_partition(
        &mut self,
        files: Arc<Vec<FileChange>>,
        partition: Option<Arc<Partition>>,
        flags: Vec<FileFlags>,
        cx: &mut Context<Self>,
    ) {
        self.builds += 1;
        self.build(files, partition, flags, cx);
        cx.notify();
    }

    fn build(
        &mut self,
        files: Arc<Vec<FileChange>>,
        partition: Option<Arc<Partition>>,
        mut flags: Vec<FileFlags>,
        cx: &mut Context<Self>,
    ) {
        flags.resize(files.len(), FileFlags::default());
        self.files = files;
        self.flags = Arc::new(flags);
        self.partition = partition;
        let was_open = self.panels.open_key().cloned();
        let mut old: Vec<Option<PanelTree>> = self.panels.take().into_iter().map(Some).collect();
        let specs = PanelSpec::of(self.partition.as_deref(), self.files.len());
        let mut built = Vec::with_capacity(specs.len());
        for spec in specs {
            let reused = old
                .iter_mut()
                .find(|p| p.as_ref().is_some_and(|p| *p.key() == spec.key))
                .and_then(Option::take);
            let panel = match reused {
                Some(mut panel) => {
                    panel.reset(spec, &self.files);
                    panel
                }
                None => PanelTree::new(spec, &self.files, cx),
            };
            built.push(panel);
        }
        if let Some(expanded) = self.restored.take() {
            for panel in &mut built {
                panel.set_expanded(&expanded);
            }
        }
        self.jumped = None;
        self.current = (!self.files.is_empty()).then(|| self.viewport.read(cx).anchor().file_idx);
        self.panels.replace(built, self.files.len());
        // A crossing (the top file in another panel than last seen: the
        // first partition, a restored position, a repartition moving it)
        // opens the top file's panel. Otherwise the open panel stays, else
        // the top file's opens, else the first ([`FilesPanel::ensure_open_shown`]).
        let followed = self.current.and_then(|f| self.key_of(f));
        let mut keys = [&followed, &was_open];
        if followed == self.followed {
            keys.reverse();
        }
        if let Some(ix) = keys
            .into_iter()
            .flatten()
            .find_map(|k| self.panels.index_of(k))
        {
            self.panels.set_open(ix);
        }
        self.followed = followed;
        // A selected folder stays only in the panel it was selected in.
        if !matches!(self.selected, Some(ItemId::Dir(_)))
            || self.panels.open_key() != was_open.as_ref()
        {
            self.selected = None;
        }
        self.rebuild(cx);
    }

    /// The key of the panel holding file `idx`.
    fn key_of(&self, idx: u32) -> Option<PanelKey> {
        let ix = self.panels.panel_of(idx)?;
        Some(self.panels.panels()[ix].key().clone())
    }

    /// Every file's review state, as last pushed.
    pub fn file_flags(&self) -> &[FileFlags] {
        &self.flags
    }

    /// Replaces every file's review state (one entry per file, in file
    /// order; missing entries are all-false).
    pub fn set_file_flags(&mut self, mut flags: Vec<FileFlags>, cx: &mut Context<Self>) {
        flags.resize(self.files.len(), FileFlags::default());
        if *self.flags == flags {
            return;
        }
        self.flags = Arc::new(flags);
        if self.filter.unviewed || self.filter.has_comments {
            self.rebuild(cx);
        } else {
            for panel in self.panels.panels_mut() {
                panel.recount(&self.flags);
            }
        }
        cx.notify();
    }

    /// Directory `dir`'s tri-state Viewed slot in the open panel's tree:
    /// on when every file below it is viewed, mixed when some are.
    pub fn folder_check(&self, dir: &str) -> Option<row::Check> {
        self.panels.open()?.folder_check(dir)
    }

    pub fn filters(&self) -> &TreeFilter {
        &self.filter
    }

    /// Applies `filter` to every panel (the text box shows `filter.query`).
    pub fn set_filters(&mut self, filter: TreeFilter, window: &mut Window, cx: &mut Context<Self>) {
        let query = filter.query.clone();
        if self.filter_input.read(cx).value() != query.as_str() {
            self.filter_input
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
        self.update_filters(|f| *f = filter, cx);
    }

    /// Sets the fuzzy filter's text (and the box's).
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        filter.query = query.to_owned();
        self.set_filters(filter, window, cx);
    }

    fn set_query_text(&mut self, query: String, cx: &mut Context<Self>) {
        if self.filter.query != query {
            self.update_filters(|f| f.query = query, cx);
        }
    }

    fn update_filters(&mut self, change: impl FnOnce(&mut TreeFilter), cx: &mut Context<Self>) {
        let before = self.filter.clone();
        change(&mut self.filter);
        if self.filter != before {
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// Turns "unviewed only" on or off.
    pub fn toggle_unviewed(&mut self, cx: &mut Context<Self>) {
        self.update_filters(|f| f.unviewed = !f.unviewed, cx);
    }

    /// Turns "has comments only" on or off.
    pub fn toggle_has_comments(&mut self, cx: &mut Context<Self>) {
        self.update_filters(|f| f.has_comments = !f.has_comments, cx);
    }

    pub fn toggle_status(&mut self, status: StatusFilter, cx: &mut Context<Self>) {
        self.update_filters(
            |f| {
                if !f.statuses.remove(&status) {
                    f.statuses.insert(status);
                }
            },
            cx,
        );
    }

    pub fn toggle_extension(&mut self, ext: &str, cx: &mut Context<Self>) {
        self.update_filters(
            |f| {
                if !f.extensions.remove(ext) {
                    f.extensions.insert(ext.to_owned());
                }
            },
            cx,
        );
    }

    /// Clears the filter, the text box included; the open panel stays.
    pub fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_filters(TreeFilter::default(), window, cx);
    }

    /// Every panel's expanded directories of its unfiltered tree
    /// (compacted chains by their deepest path; a category panel's
    /// `/<category>`, then its folders as `/<category>/<path>`), panel by
    /// panel in tree order.
    pub fn expanded_dirs(&self) -> Vec<String> {
        self.panels
            .panels()
            .iter()
            .flat_map(PanelTree::expanded_dirs)
            .collect()
    }

    /// Whether every directory of every panel is expanded (the default;
    /// view state saves no expansion then).
    pub fn all_dirs_expanded(&self) -> bool {
        self.panels.panels().iter().all(PanelTree::all_expanded)
    }

    /// Expands exactly `dirs` ([`FileTree::expanded_dirs`]'s names) of the
    /// unfiltered trees (view-state restore); a category panel without its
    /// `/<category>` in them expands every folder. Before the first
    /// partition, it also waits for the panels it builds.
    pub fn set_expanded_dirs(
        &mut self,
        dirs: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        let expanded: HashSet<String> = dirs.into_iter().collect();
        for panel in self.panels.panels_mut() {
            panel.set_expanded(&expanded);
        }
        if self.builds == 0 {
            self.restored = Some(expanded);
        }
        if self.filter.is_active() {
            cx.notify();
        } else {
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// The file the tree marks as current (the viewport's top file, or the
    /// file last chosen in the tree).
    pub fn highlighted_file(&self) -> Option<u32> {
        self.current
    }

    /// The selected row's file, if a file row is selected.
    pub fn selected_file(&self) -> Option<u32> {
        match self.selected {
            Some(ItemId::File(idx)) => Some(idx),
            _ => None,
        }
    }

    /// The selected directory row's path and the files below it (as its
    /// panel shows them).
    pub fn selected_dir(&self) -> Option<(String, Vec<u32>)> {
        match &self.selected {
            Some(ItemId::Dir(path)) => {
                let node = self.model().dir(path)?;
                Some((path.clone(), node.files.clone()))
            }
            _ => None,
        }
    }

    /// Opens panel `key` (a click on its header): the diff does not move,
    /// and the panel's list keeps its scroll position (the current file's
    /// row is marked when the panel holds it).
    pub fn open_panel(&mut self, key: &PanelKey, cx: &mut Context<Self>) {
        let Some(ix) = self.panels.index_of(key) else {
            return;
        };
        if self.panels.open_key() == Some(key) || !self.panels.set_open(ix) {
            return;
        }
        self.selected = None;
        self.mark_current(false, cx);
        cx.notify();
    }

    /// Selects file `idx` in the tree (opening its panel, expanding its
    /// directories) and scrolls the viewport to it.
    pub fn select_file(&mut self, idx: u32, cx: &mut Context<Self>) {
        if idx as usize >= self.files.len() {
            return;
        }
        if let Some(ix) = self.panels.panel_of(idx) {
            self.panels.set_open(ix);
        }
        self.reveal(idx, cx);
        self.jump(idx, cx);
    }

    /// `n` in the tree: the next file of the open panel, wrapping.
    pub fn next_file(&mut self, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    /// `p` in the tree: the previous file of the open panel, wrapping.
    pub fn prev_file(&mut self, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    /// Whether the keyboard is in the tree: a list, its filter box or its
    /// filter menu. gpui-kit's `TreeState` does not expose its focus handle,
    /// so the list is found by this widget's own key context
    /// ([`KEY_CONTEXT`]), not gpui-kit's `Tree` (any tree widget has that).
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        window
            .context_stack()
            .iter()
            .any(|c| c.contains(KEY_CONTEXT))
            || self.filter_input.focus_handle(cx).is_focused(window)
            || self
                .key_menu
                .as_ref()
                .is_some_and(|m| m.view().focus_handle(cx).contains_focused(window, cx))
    }

    /// Focuses the open panel's tree (its keys: arrows, `n`/`p`, `v`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(state) = self.tree_state() {
            state.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let order = self.model().file_order();
        if order.is_empty() {
            return;
        }
        let from = self.selected_file().or(self.current);
        let pos = from.and_then(|f| order.iter().position(|&o| o == f));
        let len = order.len() as isize;
        let next = match pos {
            Some(p) => (p as isize + delta).rem_euclid(len),
            None if delta > 0 => 0,
            None => len - 1,
        };
        let idx = order[next as usize];
        self.select_file(idx, cx);
    }

    /// Scrolls the viewport to file `idx` (opening its section) and marks
    /// it current. When the viewport cannot bring its header to the top
    /// (near the end of the diff), the tree keeps marking it while it stays
    /// on screen (`jumped`); when it can, the viewport's top file rules
    /// again.
    fn jump(&mut self, idx: u32, cx: &mut Context<Self>) {
        self.current = Some(idx);
        let landed = self.viewport.update(cx, |v, cx| {
            v.scroll_to(ScrollTarget::File(idx), cx);
            let doc = v.document();
            doc.scroll_top() + 0.5 >= doc.header_top(idx)
        });
        self.jumped = (!landed).then_some(idx);
        cx.notify();
    }

    /// Selects file `idx`'s row in the open panel, expanding its
    /// directories.
    fn reveal(&mut self, idx: u32, cx: &mut Context<Self>) {
        let Some(state) = self.tree_state().cloned() else {
            return;
        };
        let id = ItemId::File(idx);
        let key: SharedString = id.to_string().into();
        self.selected = Some(id);
        state.update(cx, |s, cx| {
            s.reveal_item(&key, ScrollStrategy::Nearest, cx);
            let ix = s.index_of(&key);
            s.set_selected_index(ix, cx);
        });
    }

    /// Marks file `idx` current without moving the viewport.
    fn highlight(&mut self, idx: u32, cx: &mut Context<Self>) {
        self.current = Some(idx);
        self.mark_current(true, cx);
        cx.notify();
    }

    /// Selects the current file's row in the open panel, or the collapsed
    /// directory hiding it (scrolled into view when `reveal`); nothing when
    /// the panel does not hold it.
    fn mark_current(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let Some(state) = self.tree_state().cloned() else {
            return;
        };
        let mut candidates: Vec<ItemId> = match self.current {
            Some(idx) => self
                .model()
                .ancestors_of_file(idx)
                .into_iter()
                .map(ItemId::Dir)
                .chain([ItemId::File(idx)])
                .collect(),
            None => Vec::new(),
        };
        // The deepest row shown: the file, else the collapsed directory.
        candidates.reverse();
        let found = state.update(cx, |s, cx| {
            let hit = candidates.iter().find_map(|id| {
                let key: SharedString = id.to_string().into();
                s.index_of(&key).map(|ix| (id.clone(), ix))
            });
            match &hit {
                Some((_, ix)) => {
                    s.set_selected_index(Some(*ix), cx);
                    if reveal {
                        s.scroll_to_item(*ix, ScrollStrategy::Nearest);
                    }
                }
                None => s.set_selected_index(None, cx),
            }
            hit.map(|(id, _)| id)
        });
        self.selected = found;
    }

    /// The viewport's top file changed: a new panel's file opens that panel
    /// (a crossing, OQ-44), and the file is marked.
    fn visible_file_changed(&mut self, idx: u32, cx: &mut Context<Self>) {
        if let Some(target) = self.jumped {
            // Near the end of the diff the viewport cannot bring the chosen
            // file to its top; keep it marked while it is on screen.
            let v = self.viewport.read(cx);
            if v.display_rank(target) > v.display_rank(idx) && self.on_screen(target, cx) {
                return;
            }
            self.jumped = None;
        }
        let key = self.key_of(idx);
        if key != self.followed {
            self.followed = key;
            if let Some(ix) = self.followed.as_ref().and_then(|k| self.panels.index_of(k))
                && self.panels.open_key() != self.followed.as_ref()
                && self.panels.set_open(ix)
            {
                self.selected = None;
            }
        }
        if self.current != Some(idx) || self.selected != Some(ItemId::File(idx)) {
            self.highlight(idx, cx);
        }
    }

    fn on_screen(&self, idx: u32, cx: &App) -> bool {
        let doc = self.viewport.read(cx).document();
        doc.header_top(idx) < doc.scroll_top() + f64::from(doc.viewport_height())
    }

    /// A panel's selection changed (a click or the arrow keys): in the open
    /// panel, a file row scrolls the viewport to its file.
    fn selection_changed(&mut self, panel: &PanelKey, cx: &mut Context<Self>) {
        if self.panels.open_key() != Some(panel) {
            return;
        }
        let now = self
            .tree_state()
            .and_then(|s| s.read(cx).selected_item())
            .and_then(|item| ItemId::parse(&item.id));
        if now == self.selected {
            return;
        }
        self.selected = now.clone();
        if let Some(ItemId::File(idx)) = now {
            self.jump(idx, cx);
        }
    }

    fn expansion_changed(&mut self, panel: &PanelKey, event: &TreeEvent, cx: &mut Context<Self>) {
        let (id, expanded) = match event {
            TreeEvent::Expanded(id) => (id, true),
            TreeEvent::Collapsed(id) => (id, false),
        };
        let (Some(ItemId::Dir(path)), Some(panel)) =
            (ItemId::parse(id), self.panels.get_mut(panel))
        else {
            return;
        };
        let set = panel.collapsed_mut();
        if expanded {
            set.remove(&path);
        } else {
            set.insert(path);
        }
        cx.notify();
    }

    /// Applies the filter, counts and expansion to every panel; when the
    /// open panel has no match, the first one with a match opens. A folder
    /// the user selected stays selected while the open panel still has it
    /// (`MarkFolderViewed` acts on it); otherwise the current file is
    /// marked.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        for panel in self.panels.panels_mut() {
            panel.filter(&self.filter, &self.files, &self.flags);
            panel.recount(&self.flags);
            panel.push_items(cx);
        }
        let before = self.panels.open_key().cloned();
        self.panels.ensure_open_shown();
        let dir = match self.selected.take() {
            Some(id @ ItemId::Dir(_)) if self.panels.open_key() == before.as_ref() => {
                Some(SharedString::from(id.to_string()))
            }
            _ => None,
        };
        let kept = match (dir.as_ref(), self.tree_state().cloned()) {
            (Some(key), Some(state)) => state.update(cx, |s, cx| {
                let ix = s.index_of(key)?;
                s.set_selected_index(Some(ix), cx);
                Some(ix)
            }),
            _ => None,
        };
        if kept.is_some() {
            self.selected = dir.as_deref().and_then(ItemId::parse);
        } else {
            self.mark_current(true, cx);
        }
    }

    /// The filter menu (funnel button).
    fn filter_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let this: WeakEntity<FileTree> = cx.entity().downgrade();
        let active = self.filter.menu_active();
        Button::new("tree-filters")
            // gpui-kit bundles only its default icons (no funnel): the
            // three-line glyph reads as a list filter.
            .icon(IconName::Menu)
            .xsmall()
            .ghost()
            .selected(active)
            .tooltip("Filter files")
            .debug_selector(|| "tree-filters".into())
            .dropdown_menu(move |menu, _, cx| {
                let Some(tree) = this.upgrade() else {
                    return menu;
                };
                let t = tree.read(cx);
                // Counted when the menu opens, not on every render.
                let (f, extensions) = (t.filter.clone(), filters::extensions(&t.files));
                filters::menu(&this, f, extensions, menu)
            })
    }
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let theme = cx.theme().clone();
        let filtering = self.filter.is_active();
        self.follow_focus(window, cx);
        let open = self.panels.open_key().cloned();
        let headers = self.panels.has_headers();
        let this = cx.entity().downgrade();
        // Panels with a match, their headers, the open one's tree after its
        // header.
        let mut accordion: Vec<AnyElement> = Vec::new();
        let mut first = true;
        for panel in self.panels.panels().iter().filter(|p| p.is_shown()) {
            let is_open = open.as_ref() == Some(panel.key());
            if headers {
                accordion.push(
                    panels::header(panel, is_open, filtering, this.clone(), first, cx)
                        .into_any_element(),
                );
                first = false;
            }
            if is_open {
                accordion.push(self.body(cx));
            }
        }
        if !self.panels.open().is_some_and(PanelTree::is_shown) {
            accordion.push(self.body(cx));
        }
        // The footer counts the Changes panel's files whatever the filter
        // shows; none when every file is categorized.
        let changes: Option<&[u32]> = match self.panels.panels().first() {
            Some(p) if *p.key() == PanelKey::Changes => Some(p.files()),
            Some(_) => None,
            None => Some(&[]),
        };
        v_flex()
            .key_context(KEY_CONTEXT)
            .size_full()
            .bg(theme.sidebar)
            .child(
                // The rounded filter field, its menu at the right edge. Esc
                // in it: back to the list.
                div()
                    .flex_none()
                    .px_2()
                    .pt_1()
                    .pb_2()
                    .on_action(cx.listener(|t, _: &Escape, window, cx| t.focus(window, cx)))
                    .child(
                        div().debug_selector(|| "tree-filter".into()).child(
                            Input::new(&self.filter_input)
                                .small()
                                .cleanable(true)
                                .prefix(
                                    Icon::new(IconName::Search)
                                        .small()
                                        .text_color(theme.muted_foreground),
                                )
                                .suffix(
                                    div()
                                        .relative()
                                        .child(self.filter_menu(cx))
                                        .when_some(self.key_menu.as_ref(), |el, menu| {
                                            el.child(menu.element(Anchor::TopRight))
                                        }),
                                )
                                .min_h(px(28.))
                                .rounded(px(8.))
                                .bg(theme.tab_bar_segmented)
                                .border_color(gpui_kit::transparent_black()),
                        ),
                    ),
            )
            .children(accordion)
            .child(footer::footer(
                changes,
                self.partition.as_ref(),
                &self.viewport,
                cx,
            ))
    }
}
