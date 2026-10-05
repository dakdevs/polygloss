//! File tree and file finder (⌘P) (design §11.5, §11.8).
//!
//! - [`FileTree`]: the sidebar's Files segment, a gpui-kit `Tree`
//!   (virtualized, key context `Tree`) over the diff's files with directory
//!   chains compacted ([`model`]); rows ([`row`]) carry a Viewed checkbox,
//!   the status letter, +/− counts, open-thread and agent badges and the
//!   "changed since viewed" dot; [`filters`] hide files (unviewed, has
//!   comments, status, extension, fuzzy text). Selecting a file scrolls the
//!   viewport to it; the viewport's top file is highlighted in the tree.
//! - [`finder`]: ⌘P, every changed path ranked by `nucleo-matcher`; Enter
//!   jumps to the file.
//!
//! The filter (`tree::FocusFilter`) and ⌘P show the sidebar's Files segment
//! first ([`crate::chrome::show_files`]).
//!
//! The Viewed state is T3.7's: it pushes [`FileFlags`] with
//! [`FileTree::set_file_flags`] and handles the checkboxes' [`FileTreeEvent`]s
//! and `tree::ToggleViewed` (with [`FileTree::selected_file`] /
//! [`FileTree::selected_dir`]). View-state (T3.14) reads and restores the
//! expansion with [`FileTree::expanded_dirs`] / [`FileTree::set_expanded_dirs`].

pub mod filters;
pub mod finder;
pub mod model;
pub mod row;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, MenuItem, ParentElement as _, Render,
    ScrollStrategy, SharedString, Styled as _, Subscription, WeakEntity, Window, div, px,
};
use polygloss_diff::FileChange;
use polygloss_viewport::{DiffViewport, FileFlags, ScrollTarget, ViewportEvent};

use crate::keyboard::menu::KeyMenu;
use crate::keymap::actions::{tree as tree_actions, window as window_actions};
use crate::keymap::handlers;
use crate::live::DiffRefreshed;
use crate::review_tab::ReviewTab;
use crate::window::MenuKind;
use filters::{StatusFilter, TreeFilters};
use model::{ItemId, TreeModel};

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

/// Creates the tab's [`FileTree`]; a refresh (T3.11) gives it the new
/// files.
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let files = tab.opened.files.clone();
    let viewport = tab.viewport.clone();
    let tree = cx.new(|cx| FileTree::new(files, viewport, window, cx));
    tab.insert_extension(TreePane(tree));
    cx.subscribe_self(|tab: &mut ReviewTab, _: &DiffRefreshed, cx| {
        let Some(tree) = file_tree(tab).cloned() else {
            return;
        };
        let files = tab.opened.files.clone();
        let flags = tab.viewport.read(cx).file_flags().to_vec();
        tree.update(cx, |t, cx| t.set_files(files, flags, cx));
    })
    .detach();
}

/// The file tree pane.
pub fn render_pane(
    tab: &ReviewTab,
    _window: &mut Window,
    _cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    file_tree(tab).map(|t| t.clone().into_any_element())
}

/// What the tree asks its host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTreeEvent {
    /// A file's Viewed checkbox was clicked.
    ToggleViewed(u32),
    /// A directory's Viewed checkbox was clicked: every file below it (as
    /// the tree shows them).
    ToggleFolderViewed { dir: String, files: Vec<u32> },
}

/// One visible row of the tree (tests and the view-state).
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
    state: Entity<TreeState>,
    filter_input: Entity<InputState>,
    /// Every file's tree.
    full: Arc<TreeModel>,
    /// The tree of the files the filters keep, while any filter is on.
    filtered: Option<Arc<TreeModel>>,
    flags: Arc<Vec<FileFlags>>,
    filters: TreeFilters,
    /// Viewed files and all files per directory of the shown tree.
    dir_viewed: Arc<HashMap<String, (u32, u32)>>,
    /// Directories collapsed in the unfiltered tree (all start expanded).
    collapsed: HashSet<String>,
    /// Directories collapsed while filtering (reset by each filter change).
    filter_collapsed: HashSet<String>,
    /// The selection as last seen, to tell the user's changes from ours.
    selected: Option<ItemId>,
    /// The file the tree marks as current.
    current: Option<u32>,
    /// The file last jumped to from the tree, while the viewport shows it.
    jumped: Option<u32>,
    /// The filter menu, when opened from the keyboard (`f`).
    key_menu: Option<KeyMenu>,
    _subscriptions: Vec<Subscription>,
}

/// The file tree's own key context, around its list, filter box and
/// filter menu (no bindings use it: [`FileTree::contains_focus`] does).
pub const KEY_CONTEXT: &str = "FileTree";

impl EventEmitter<FileTreeEvent> for FileTree {}

impl FileTree {
    /// The tree of `files`, scrolling `viewport`.
    pub fn new(
        files: Arc<Vec<FileChange>>,
        viewport: Entity<DiffViewport>,
        window: &mut Window,
        cx: &mut Context<FileTree>,
    ) -> FileTree {
        let full = Arc::new(TreeModel::build(
            files
                .iter()
                .enumerate()
                .map(|(i, f)| (i as u32, f.display_path())),
        ));
        // The tree walks files depth-first, the viewport in diff order;
        // `n`/`p` and the "file above/below" checks rely on them agreeing,
        // which holds while git lists each directory's files together
        // (`tree_order_matches_diff_order`).
        debug_assert!(
            full.file_order().windows(2).all(|w| w[0] < w[1]),
            "tree order differs from diff order"
        );
        let state = cx.new(|cx| TreeState::new(cx).items(tree_items(&full, &HashSet::new())));
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files…"));
        let subscriptions = vec![
            cx.observe(&state, |t: &mut FileTree, _, cx| t.selection_changed(cx)),
            cx.subscribe(&state, |t: &mut FileTree, _, event: &TreeEvent, cx| {
                t.expansion_changed(event, cx)
            }),
            cx.subscribe(
                &viewport,
                |t: &mut FileTree, _, event: &ViewportEvent, cx| {
                    if let ViewportEvent::VisibleFileChanged(idx) = *event {
                        t.visible_file_changed(idx, cx);
                    }
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
        let flags = Arc::new(vec![FileFlags::default(); files.len()]);
        let mut tree = FileTree {
            files,
            viewport,
            state,
            filter_input,
            key_menu: None,
            full,
            filtered: None,
            flags,
            filters: TreeFilters::default(),
            dir_viewed: Arc::default(),
            collapsed: HashSet::new(),
            filter_collapsed: HashSet::new(),
            selected: None,
            current: None,
            jumped: None,
            _subscriptions: subscriptions,
        };
        tree.dir_viewed = Arc::new(tree.count_viewed());
        let top = tree.viewport.read(cx).anchor().file_idx;
        if !tree.files.is_empty() {
            tree.highlight(top, cx);
        }
        tree
    }

    /// The tree shown (filtered or not).
    pub fn model(&self) -> &TreeModel {
        self.filtered.as_deref().unwrap_or(&self.full)
    }

    /// The gpui-kit tree state (selection, focus, scroll).
    pub fn tree_state(&self) -> &Entity<TreeState> {
        &self.state
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
        let (f, extensions) = (self.filters.clone(), filters::extensions(&self.files));
        let menu = KeyMenu::open(
            |t: &mut FileTree| Some(&mut t.key_menu),
            self.key_menu.take(),
            move |menu, _, _| build_filter_menu(&tree, f, extensions, menu),
            window,
            cx,
        );
        self.key_menu = Some(menu);
        cx.notify();
    }

    /// The visible rows, top to bottom.
    pub fn rows(&self, cx: &App) -> Vec<TreeRow> {
        let state = self.state.read(cx);
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

    /// Shows another file list (the diff was refreshed, T3.11) with its
    /// files' review state: the same filters, the same collapsed
    /// directories (by path) and a selected folder that still exists; the
    /// viewport's top file is marked.
    pub fn set_files(
        &mut self,
        files: Arc<Vec<FileChange>>,
        mut flags: Vec<FileFlags>,
        cx: &mut Context<Self>,
    ) {
        self.full = Arc::new(TreeModel::build(
            files
                .iter()
                .enumerate()
                .map(|(i, f)| (i as u32, f.display_path())),
        ));
        self.files = files;
        flags.resize(self.files.len(), FileFlags::default());
        self.flags = Arc::new(flags);
        let dirs: HashSet<String> = self.full.dir_paths().into_iter().collect();
        self.collapsed.retain(|d| dirs.contains(d));
        if !matches!(self.selected, Some(ItemId::Dir(_))) {
            self.selected = None;
        }
        self.jumped = None;
        self.current = (!self.files.is_empty()).then(|| self.viewport.read(cx).anchor().file_idx);
        self.rebuild(cx);
        cx.notify();
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
        if self.filters.unviewed || self.filters.has_comments {
            self.rebuild(cx);
        } else {
            self.dir_viewed = Arc::new(self.count_viewed());
        }
        cx.notify();
    }

    /// Directory `dir`'s tri-state checkbox (of the tree shown): checked when
    /// every file below it is viewed, mixed when some are.
    pub fn folder_check(&self, dir: &str) -> Option<row::Check> {
        self.dir_viewed
            .get(dir)
            .map(|&(viewed, total)| row::Check::of(viewed, total))
    }

    pub fn filters(&self) -> &TreeFilters {
        &self.filters
    }

    /// Applies `filters` (the text box shows `filters.query`).
    pub fn set_filters(
        &mut self,
        filters: TreeFilters,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = filters.query.clone();
        if self.filter_input.read(cx).value() != query.as_str() {
            self.filter_input
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
        self.update_filters(|f| *f = filters, cx);
    }

    /// Sets the fuzzy filter's text (and the box's).
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut filters = self.filters.clone();
        filters.query = query.to_owned();
        self.set_filters(filters, window, cx);
    }

    fn set_query_text(&mut self, query: String, cx: &mut Context<Self>) {
        if self.filters.query != query {
            self.update_filters(|f| f.query = query, cx);
        }
    }

    fn update_filters(&mut self, change: impl FnOnce(&mut TreeFilters), cx: &mut Context<Self>) {
        let before = self.filters.clone();
        change(&mut self.filters);
        if self.filters != before {
            self.filter_collapsed.clear();
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

    /// Clears every filter, the text box included.
    pub fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_filters(TreeFilters::default(), window, cx);
    }

    /// The expanded directories of the unfiltered tree (compacted chains by
    /// their deepest path), in tree order.
    pub fn expanded_dirs(&self) -> Vec<String> {
        self.full
            .dir_paths()
            .into_iter()
            .filter(|d| !self.collapsed.contains(d))
            .collect()
    }

    /// Whether every directory of the unfiltered tree is expanded (the
    /// default; view state saves no expansion then).
    pub fn all_dirs_expanded(&self) -> bool {
        self.collapsed.is_empty()
    }

    /// Expands exactly `dirs` of the unfiltered tree (view-state restore).
    pub fn set_expanded_dirs(
        &mut self,
        dirs: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        let expanded: HashSet<String> = dirs.into_iter().collect();
        self.collapsed = self
            .full
            .dir_paths()
            .into_iter()
            .filter(|d| !expanded.contains(d))
            .collect();
        if self.filtered.is_none() {
            self.rebuild(cx);
        }
        cx.notify();
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

    /// The selected directory row's path and the files below it (as shown).
    pub fn selected_dir(&self) -> Option<(String, Vec<u32>)> {
        match &self.selected {
            Some(ItemId::Dir(path)) => {
                let node = self.model().dir(path)?;
                Some((path.clone(), node.files.clone()))
            }
            _ => None,
        }
    }

    /// Selects file `idx` in the tree (expanding its directories) and
    /// scrolls the viewport to it.
    pub fn select_file(&mut self, idx: u32, cx: &mut Context<Self>) {
        if idx as usize >= self.files.len() {
            return;
        }
        self.reveal(idx, cx);
        self.jump(idx, cx);
    }

    /// `n` in the tree: the next file (tree order), wrapping.
    pub fn next_file(&mut self, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    /// `p` in the tree: the previous file (tree order), wrapping.
    pub fn prev_file(&mut self, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    /// Whether the keyboard is in the tree: its list, its filter box or its
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

    /// Focuses the tree (its keys: arrows, `n`/`p`, `v`).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |s, cx| s.focus(window, cx));
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

    /// Scrolls the viewport to file `idx` and marks it current. When the
    /// viewport cannot bring its header to the top (near the end of the
    /// diff), the tree keeps marking it while it stays on screen
    /// (`jumped`); when it can, the viewport's top file rules again.
    fn jump(&mut self, idx: u32, cx: &mut Context<Self>) {
        self.current = Some(idx);
        let landed = self.viewport.update(cx, |v, cx| {
            v.scroll_to(ScrollTarget::File(idx), cx);
            let doc = v.document();
            doc.scroll_top() + 0.5 >= doc.file_top(idx)
        });
        self.jumped = (!landed).then_some(idx);
        cx.notify();
    }

    /// Selects file `idx`'s row, expanding its directories.
    fn reveal(&mut self, idx: u32, cx: &mut Context<Self>) {
        let id = ItemId::File(idx);
        let key: SharedString = id.to_string().into();
        self.selected = Some(id);
        self.state.update(cx, |s, cx| {
            s.reveal_item(&key, ScrollStrategy::Nearest, cx);
            let ix = s.index_of(&key);
            s.set_selected_index(ix, cx);
        });
    }

    /// Marks file `idx` current without moving the viewport: selects its
    /// row, or the collapsed directory hiding it.
    fn highlight(&mut self, idx: u32, cx: &mut Context<Self>) {
        self.current = Some(idx);
        let mut candidates: Vec<ItemId> = self
            .model()
            .ancestors_of_file(idx)
            .into_iter()
            .map(ItemId::Dir)
            .collect();
        candidates.push(ItemId::File(idx));
        let found = self.state.update(cx, |s, cx| {
            // The deepest row shown: the file, else the collapsed directory.
            let hit = candidates.iter().rev().find_map(|id| {
                let key: SharedString = id.to_string().into();
                s.index_of(&key).map(|ix| (id.clone(), ix))
            });
            match &hit {
                Some((_, ix)) => {
                    s.set_selected_index(Some(*ix), cx);
                    s.scroll_to_item(*ix, ScrollStrategy::Nearest);
                }
                None => s.set_selected_index(None, cx),
            }
            hit.map(|(id, _)| id)
        });
        self.selected = found;
        cx.notify();
    }

    /// The viewport's top file changed.
    fn visible_file_changed(&mut self, idx: u32, cx: &mut Context<Self>) {
        if let Some(target) = self.jumped {
            // Near the end of the diff the viewport cannot bring the chosen
            // file to its top; keep it marked while it is on screen.
            if target > idx && self.on_screen(target, cx) {
                return;
            }
            self.jumped = None;
        }
        if self.current != Some(idx) || self.selected != Some(ItemId::File(idx)) {
            self.highlight(idx, cx);
        }
    }

    fn on_screen(&self, idx: u32, cx: &App) -> bool {
        let doc = self.viewport.read(cx).document();
        doc.file_top(idx) < doc.scroll_top() + f64::from(doc.viewport_height())
    }

    /// The tree's selection changed (a click or the arrow keys): a file row
    /// scrolls the viewport to its file.
    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        let now = self
            .state
            .read(cx)
            .selected_item()
            .and_then(|item| ItemId::parse(&item.id));
        if now == self.selected {
            return;
        }
        self.selected = now.clone();
        if let Some(ItemId::File(idx)) = now {
            self.jump(idx, cx);
        }
    }

    fn expansion_changed(&mut self, event: &TreeEvent, cx: &mut Context<Self>) {
        let (id, expanded) = match event {
            TreeEvent::Expanded(id) => (id, true),
            TreeEvent::Collapsed(id) => (id, false),
        };
        let Some(ItemId::Dir(path)) = ItemId::parse(id) else {
            return;
        };
        let set = if self.filtered.is_some() {
            &mut self.filter_collapsed
        } else {
            &mut self.collapsed
        };
        if expanded {
            set.remove(&path);
        } else {
            set.insert(path);
        }
        cx.notify();
    }

    /// Rebuilds the shown tree from the files, the filters and the
    /// expansion, keeping the current file marked.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.filtered = self.filters.is_active().then(|| {
            let keep = self.filters.apply(&self.files, &self.flags);
            Arc::new(TreeModel::build(
                keep.iter()
                    .map(|&i| (i, self.files[i as usize].display_path())),
            ))
        });
        let collapsed = if self.filtered.is_some() {
            &self.filter_collapsed
        } else {
            &self.collapsed
        };
        let items = tree_items(self.model(), collapsed);
        self.dir_viewed = Arc::new(self.count_viewed());
        // A folder the user selected stays selected while the new tree has
        // it (`MarkFolderViewed` acts on it); otherwise mark the current file.
        let dir = match self.selected.take() {
            Some(id @ ItemId::Dir(_)) => Some(SharedString::from(id.to_string())),
            _ => None,
        };
        let kept = self.state.update(cx, |s, cx| {
            s.set_items(items, cx);
            let ix = dir.as_ref().and_then(|key| s.index_of(key))?;
            s.set_selected_index(Some(ix), cx);
            Some(ix)
        });
        if kept.is_some() {
            self.selected = dir.as_deref().and_then(ItemId::parse);
        } else if let Some(current) = self.current {
            self.highlight(current, cx);
        }
    }

    /// Viewed and total files per directory of the shown tree.
    fn count_viewed(&self) -> HashMap<String, (u32, u32)> {
        self.model()
            .nodes()
            .iter()
            .filter(|n| n.is_dir())
            .map(|n| {
                let viewed = n
                    .files
                    .iter()
                    .filter(|&&f| self.flags.get(f as usize).is_some_and(|x| x.viewed))
                    .count() as u32;
                (n.path.clone(), (viewed, n.files.len() as u32))
            })
            .collect()
    }

    fn row_ctx(&self, cx: &Context<Self>) -> row::RowCtx {
        row::RowCtx {
            files: self.files.clone(),
            flags: self.flags.clone(),
            dir_viewed: self.dir_viewed.clone(),
            model: self.filtered.clone().unwrap_or_else(|| self.full.clone()),
            viewport: self.viewport.clone(),
            tree: cx.entity().downgrade(),
        }
    }

    /// The filter menu (funnel button).
    fn filter_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let this: WeakEntity<FileTree> = cx.entity().downgrade();
        let active = self.filters.menu_active();
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
                let (f, extensions) = (t.filters.clone(), filters::extensions(&t.files));
                build_filter_menu(&this, f, extensions, menu)
            })
    }
}

/// A filter menu item's effect.
type FilterToggle = Box<dyn Fn(&mut FileTree, &mut Context<FileTree>)>;

/// The filter menu of `tree` (the funnel button's, and `f`'s): Unviewed,
/// Has comments, the statuses and, with more than one, the extensions, each
/// checked when on; "Clear filters" while any is on.
/// `f` and `extensions` are the tree's filters and file extensions as the
/// menu opens.
fn build_filter_menu(
    tree: &WeakEntity<FileTree>,
    f: TreeFilters,
    extensions: Vec<(String, usize)>,
    mut menu: PopupMenu,
) -> PopupMenu {
    let item = |label: &str, checked: bool, act: FilterToggle| {
        let tree = tree.clone();
        PopupMenuItem::new(label.to_owned())
            .checked(checked)
            .on_click(move |_, _, cx| {
                if let Some(tree) = tree.upgrade() {
                    tree.update(cx, |t, cx| act(t, cx));
                }
            })
    };
    menu = menu
        .item(item(
            "Unviewed",
            f.unviewed,
            Box::new(|t, cx| t.toggle_unviewed(cx)),
        ))
        .item(item(
            "Has comments",
            f.has_comments,
            Box::new(|t, cx| t.toggle_has_comments(cx)),
        ))
        .separator()
        .label("Status");
    for status in StatusFilter::ALL {
        menu = menu.item(item(
            status.label(),
            f.statuses.contains(&status),
            Box::new(move |t, cx| t.toggle_status(status, cx)),
        ));
    }
    if extensions.len() > 1 {
        menu = menu.separator().label("Extension");
        for (ext, count) in &extensions {
            let label = if ext.is_empty() {
                format!("No extension ({count})")
            } else {
                format!(".{ext} ({count})")
            };
            let e = ext.clone();
            menu = menu.item(item(
                &label,
                f.extensions.contains(ext),
                Box::new(move |t, cx| t.toggle_extension(&e, cx)),
            ));
        }
    }
    if f.menu_active() {
        let tree = tree.clone();
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Clear filters").on_click(move |_, window, cx| {
                    if let Some(tree) = tree.upgrade() {
                        tree.update(cx, |t, cx| t.clear_filters(window, cx));
                    }
                }),
            );
    }
    menu.max_h(px(420.)).scrollable(true)
}

/// gpui-kit tree items for `model`, directories expanded unless in
/// `collapsed`.
fn tree_items(model: &TreeModel, collapsed: &HashSet<String>) -> Vec<TreeItem> {
    fn build_item(model: &TreeModel, id: model::NodeId, collapsed: &HashSet<String>) -> TreeItem {
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

impl Render for FileTree {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let total = self.files.len();
        let shown = self.model().file_order().len();
        let count: SharedString = if self.filtered.is_some() {
            format!("{shown} of {total}").into()
        } else {
            total.to_string().into()
        };
        let ctx = row::RowCtx::shared(self.row_ctx(cx));
        let body: AnyElement = if shown == 0 {
            let this = cx.entity().downgrade();
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if total == 0 {
                    "No files changed"
                } else {
                    "No files match the filters"
                })
                .when(total > 0, |el| {
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
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    tree(&self.state, move |ix, entry, selected, window, cx| {
                        row::render(&ctx, ix, entry, selected, window, cx)
                    })
                    .size_full(),
                )
                .into_any_element()
        };
        v_flex()
            .key_context(KEY_CONTEXT)
            .size_full()
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(32.))
                    .pl_3()
                    .pr_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child("FILES")
                    .child(
                        div()
                            .debug_selector(|| "tree-count".into())
                            .font_normal()
                            .child(count),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .relative()
                            .child(self.filter_menu(cx))
                            .when_some(self.key_menu.as_ref(), |el, menu| {
                                el.child(menu.element(Anchor::TopRight))
                            }),
                    ),
            )
            .child(
                // Esc in the filter box: back to the list.
                div()
                    .flex_none()
                    .px_2()
                    .py_1p5()
                    .on_action(cx.listener(|t, _: &Escape, window, cx| t.focus(window, cx)))
                    .child(
                        Input::new(&self.filter_input)
                            .small()
                            .cleanable(true)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            ),
                    ),
            )
            .child(body)
    }
}
