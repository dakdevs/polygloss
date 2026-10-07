//! The Files segment's accordion (design §11.5, OQ-44): **Changes** (the
//! uncategorized files), then one panel per non-empty enabled category, in
//! category order, built from the tab's [`Partition`]. When every file is
//! categorized, Changes is left out.
//!
//! - **One open at a time:** each panel is a [`PanelTree`] with its own
//!   gpui-kit `TreeState` (virtualized, scrolling on its own) and its own
//!   collapsed folders; [`FilesPanel`] holds them and which one is open.
//!   Only the open panel's tree is rendered; the others are their headers.
//!   The follow, jumps, repartitions and the filter switch at once
//!   ([`FilesPanel::set_open`]); the keyboard in the old list moves to the
//!   new one.
//! - **The switch (M9, ADR-0030):** a header click
//!   ([`FilesPanel::open_by`], the pointer) glides: on the commit frame the
//!   model, layout and keyboard are the new panel's, and one [`Track`] of
//!   `FilesPanel` paints the way there over `reveal(H)` on SLIDE. Both
//!   bodies are laid out at the body's height H and clipped from their
//!   headers' bottom edges, the opening one's clip growing 0 → H while the
//!   closing one's shrinks (their sum is H, from one quantized value); the
//!   headers between translate by the closing clip (down when the panel
//!   opening is above); both chevrons turn on the track. The closing body
//!   stays live (its list keeps its element state and scroll offset) in a
//!   0 pt slot under its header, inert under an occluding layer. Reduce
//!   Motion swaps at once and fades the incoming list in over QUICK; Off and
//!   the keyboard snap. A third header click settles the swap and starts its
//!   own; the window's settling ([`motion::settle`]) freezes and settles it.
//! - **Headers:** chevron, the category's icon, its title and file count
//!   ("3 of 12" while the filter is on), then, over the panel's files, the
//!   changed-since-viewed dot, the open-thread count and the agent badge. A
//!   click opens the panel and gives its list the keyboard; the list keeps
//!   its scroll position. A lone Changes panel (nothing categorized) shows
//!   no header: it is the plain tree of the reference.
//! - **Follow** (OQ-44): the viewport's top file crossing into another
//!   panel's files opens that panel (`FileTree::visible_file_changed`,
//!   then `FileTree::follow_focus` for the keyboard; a partition decides in
//!   `FileTree::build`).
//! - **Filter:** the segment's one [`TreeFilter`] applies to every panel;
//!   each reports `(matches, total)`; a panel without a match is hidden.
//! - **Folders** cover their own panel's files only (a `src/` in Changes
//!   and one in Tests are two rows with two Viewed slots).
//!
//! - **Spacing** (ADR-0031): a header is a row of its list, `height::MD`,
//!   its highlight on the sidebar's edge like the rows'; its chevron box at
//!   `ICON_LEAD`, the category's icon (the label's leading box) and the
//!   title each `gap::ICON_LABEL` later.
//!
//! Debug selectors (`key` = [`PanelKey`]'s text: `changes`, `tests`,
//! `custom:tokens`): `tree-panel-{key}` (the header), `tree-panel-chevron-{key}`
//! (its chevron's box), `tree-panel-body-{key}` (its body's visible part,
//! the clip), `tree-panel-title-{key}:
//! {title}`, `tree-panel-count-{key}: {count}`, `tree-panel-threads-{key}:
//! {n}`, `tree-panel-agent-{key}`, `tree-panel-changed-{key}`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::button::Button;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    WeakEntity, Window, canvas, div, point, px, radians,
};
use polygloss_core::categories::CategoryId;
use polygloss_diff::FileChange;
use polygloss_viewport::{BandFlags, FileFlags, group_digits};

use super::filters::TreeFilter;
use super::model::{ItemId, NodeId, TreeModel};
use super::row::Check;
use super::{FileTree, KEY_CONTEXT, row};
use crate::categories::Partition;
use crate::motion::{
    self, Initiator, Motion, Recorded, Reduced, ReducedPlay, Settle, Track, tokens, wrap,
};
use crate::review_tab::toolbar::tooltip;
use crate::space::{TextStyleExt as _, edge, gap, height, pad, radius, size, text};

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
    /// (as before panels existed), a category's as `/<category>/<path>`
    /// (no git path starts with `/`, so it is never one of Changes').
    fn dir_key(&self, path: &str) -> String {
        match self {
            PanelKey::Changes => path.to_owned(),
            PanelKey::Category(id) => format!("/{id}/{path}"),
        }
    }

    /// A category panel's marker, `/<category>`: view state records its
    /// folders (Changes' are always recorded).
    fn marker(&self) -> Option<String> {
        match self {
            PanelKey::Changes => None,
            PanelKey::Category(id) => Some(format!("/{id}")),
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
    /// them ([`PanelKey::dir_key`]), in tree order, after a category
    /// panel's marker ([`PanelKey::marker`]).
    pub(super) fn expanded_dirs(&self) -> impl Iterator<Item = String> + '_ {
        self.key.marker().into_iter().chain(
            self.full
                .dir_paths()
                .into_iter()
                .filter(|d| !self.collapsed.contains(d))
                .map(|d| self.key.dir_key(&d)),
        )
    }

    /// Expands exactly the folders of `expanded` (view state's names); a
    /// category panel without its marker in them (a state saved before
    /// panels, or before the category was on) expands every folder.
    pub(super) fn set_expanded(&mut self, expanded: &HashSet<String>) {
        if self
            .key
            .marker()
            .is_some_and(|marker| !expanded.contains(&marker))
        {
            self.collapsed.clear();
            return;
        }
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

/// M9, the panel switch by pointer (ADR-0030): `reveal(H)` on SLIDE, the
/// chevrons on its track; under Reduce Motion the switch is instant and the
/// incoming list fades in.
fn switch_motion(height: f32) -> Motion {
    let enter = tokens::reveal(height, height);
    Motion {
        enter,
        exit: tokens::exit(enter),
        easing: tokens::slide,
        animates: &[Initiator::Pointer],
        reduced: Reduced {
            enter: ReducedPlay::Fade,
            exit: ReducedPlay::Snap,
        },
    }
}

/// A switch under way: the open panel is the one opening.
struct Swap {
    /// The panel closing: its body stays mounted, inert, until the swap
    /// settles.
    closing: PanelKey,
    /// The opening body's share of `height`, 0 → 1.
    track: Track,
    /// H, both bodies' height.
    height: f32,
}

/// One frame of a [`Swap`], from its one quantized clip.
#[derive(Clone, Copy, Debug)]
struct SwapFrame {
    opening: usize,
    closing: usize,
    height: f32,
    /// The opening body's clip; the closing body's is the rest of H.
    open: f32,
    /// The opening list's opacity (a Reduced fade).
    opacity: f32,
    /// The track's value, which turns both chevrons.
    progress: f32,
}

impl SwapFrame {
    fn closing_clip(&self) -> f32 {
        self.height - self.open
    }

    /// How far panel `ix`'s header (and the body hanging from it) is painted
    /// from its place: the headers after the upper panel, down to the lower
    /// one, sit below the upper body's clip instead of its settled height.
    fn offset(&self, ix: usize) -> f32 {
        let upper = self.opening.min(self.closing);
        let lower = self.opening.max(self.closing);
        if ix <= upper || ix > lower {
            0.0
        } else if upper == self.closing {
            self.closing_clip()
        } else {
            -self.closing_clip()
        }
    }

    /// How open panel `ix`'s chevron is drawn (1 open), if it turns.
    fn chevron(&self, ix: usize) -> Option<f32> {
        if ix == self.opening {
            Some(self.progress)
        } else if ix == self.closing {
            Some(1.0 - self.progress)
        } else {
            None
        }
    }
}

/// The panels of a tree and the open one.
#[derive(Default)]
pub struct FilesPanel {
    panels: Vec<PanelTree>,
    open: Option<usize>,
    /// Each file's panel, by `file_idx` (`u32::MAX`: none).
    owner: Vec<u32>,
    /// The switch under way (M9).
    swap: Option<Swap>,
    /// The open body's height as last laid out (H).
    body_height: f32,
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

    /// Opens panel `ix` at once (the follow, jumps, repartitions, the
    /// filter: Programmatic), ending a switch under way; `false` when it
    /// does not show.
    pub(super) fn set_open(&mut self, ix: usize) -> bool {
        let shown = self.panels.get(ix).is_some_and(PanelTree::is_shown);
        if shown && self.open != Some(ix) {
            self.open = Some(ix);
            self.swap = None;
        }
        shown
    }

    /// A user opens panel `key` (a header click: [`Initiator::Pointer`]):
    /// its list takes the keyboard, and from the open panel it glides when
    /// `initiator` animates M9 (any switch under way settles first). `true`
    /// when the open panel changed.
    pub fn open_by(
        &mut self,
        key: &PanelKey,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(ix) = self.index_of(key).filter(|&ix| self.panels[ix].is_shown()) else {
            return false;
        };
        self.panels[ix]
            .state
            .update(cx, |s, cx| s.focus(window, cx));
        if self.open == Some(ix) {
            return false;
        }
        let closing = self.open().filter(|p| p.is_shown()).map(|p| p.key.clone());
        self.open = Some(ix);
        self.swap = None;
        if let Some(closing) = closing
            && self.body_height > 0.0
        {
            let mut track = Track::new(0.0, 1.0);
            let now = cx.background_executor().now();
            let motion = switch_motion(self.body_height);
            track.retarget(1.0, &motion, initiator, motion::policy(cx), now);
            if !track.is_settled() {
                self.swap = Some(Swap {
                    closing,
                    track,
                    height: self.body_height,
                });
            }
        }
        true
    }

    /// Whether a switch is under way (M9).
    pub fn is_swapping(&self) -> bool {
        self.swap.is_some()
    }

    /// The open body's height as last laid out: H, the distance a switch
    /// travels.
    pub fn body_height(&self) -> f32 {
        self.body_height
    }

    /// Samples the switch for this frame (requesting the next one while it
    /// runs): `None` once it settled, or when a change since made it moot
    /// (either panel gone or hidden).
    fn sample_swap(&mut self, window: &mut Window, cx: &App) -> Option<SwapFrame> {
        let Some(opening) = self.open else {
            self.swap = None;
            return None;
        };
        let closing = self.swap.as_ref().and_then(|s| self.index_of(&s.closing));
        let live = closing.filter(|&c| {
            c != opening && self.panels[c].is_shown() && self.panels[opening].is_shown()
        });
        let (Some(closing), Some(swap)) = (live, self.swap.as_mut()) else {
            self.swap = None;
            return None;
        };
        let sample = motion::sample(&mut swap.track, window, cx);
        if sample.settled {
            self.swap = None;
            return None;
        }
        Some(SwapFrame {
            opening,
            closing,
            height: swap.height,
            open: motion::quantize(swap.height * sample.value, window.scale_factor()),
            opacity: sample.opacity,
            progress: sample.value,
        })
    }

    /// A mouse down: the switch holds where it is.
    fn freeze(&mut self, now: Instant) {
        if let Some(swap) = &mut self.swap {
            swap.track.freeze(now);
        }
    }

    /// A settling input ends the switch (`Settle::Frozen`: only if it is
    /// still frozen).
    fn settle(&mut self, which: Settle) {
        if which == Settle::All || self.swap.as_ref().is_some_and(|s| s.track.is_frozen()) {
            self.swap = None;
        }
    }

    /// The open panel when it shows, else the first one that does.
    pub(super) fn ensure_open_shown(&mut self) {
        if self.open().is_some_and(PanelTree::is_shown) {
            return;
        }
        if let Some(ix) = self.panels.iter().position(PanelTree::is_shown) {
            self.set_open(ix);
        }
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
        self.swap = None;
    }

    /// Takes every panel out (to reuse their list states).
    pub(super) fn take(&mut self) -> Vec<PanelTree> {
        self.open = None;
        self.swap = None;
        std::mem::take(&mut self.panels)
    }

    /// Whether headers show: more than one panel, or a category's alone.
    pub(super) fn has_headers(&self) -> bool {
        self.panels.len() > 1 || self.panels.iter().any(|p| p.key != PanelKey::Changes)
    }
}

/// A panel's header, its chevron `open` (0 closed, 1 open; between while it
/// turns): a click opens the panel.
fn header(
    panel: &PanelTree,
    open: f32,
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
    let radius = radius::for_height(height::MD);
    let chevron = format!("tree-panel-chevron-{key}");
    let angle = tokens::CHEVRON_CLOSED_DEG * (1.0 - open);
    motion::report(
        || Recorded {
            id: SharedString::from(chevron.clone()).into(),
            offset: Default::default(),
            opacity: 1.0,
            rotation: angle,
        },
        cx,
    );
    let control = h_flex()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .relative()
        .h(px(height::MD))
        .pl(px(pad::ICON_LEAD))
        .pr(px(pad::TEXT))
        .gap(px(gap::ICON_LABEL))
        .rounded(px(radius))
        .text_style(text::UI)
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            if let Some(tree) = tree.upgrade() {
                tree.update(cx, |t, cx| {
                    t.open_panel_by(&open_key, Initiator::Pointer, window, cx)
                });
            }
        })
        .child(crate::chrome::ink_layer(radius, cx))
        .child(
            div()
                .debug_selector(move || chevron)
                .flex_none()
                .flex()
                .child(
                    Icon::new(IconName::ChevronDown)
                        .with_size(px(size::ICON_XS))
                        .rotate(radians(angle.to_radians()))
                        .text_color(theme.muted_foreground),
                ),
        )
        .child(
            Icon::empty()
                .path(panel.icon.clone())
                .with_size(px(size::ICON_SM))
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
                .text_style(text::SMALL)
                .font_features(crate::chrome::tabular_figures())
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
                    .size(px(size::DOT))
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
                    .px(px(pad::BADGE_X))
                    .rounded_full()
                    .bg(theme.muted)
                    .text_style(text::SMALL)
                    .font_features(crate::chrome::tabular_figures())
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
        });
    // The separator spans the sidebar; the header's highlight sits on the
    // sidebar's edge.
    div()
        .flex_none()
        .w_full()
        .px(px(edge::SIDEBAR))
        .when(!first, |el| el.border_t_1().border_color(theme.border))
        .child(control)
}

/// Where a body is drawn on this frame.
#[derive(Clone, Copy, Debug)]
enum Layer {
    /// Open, nothing moving: the whole slot.
    Rest,
    /// Opening (M9): `clip` tall, `top` below its slot's top (its header's
    /// offset), the list `height` tall at `opacity`.
    Opening {
        top: f32,
        clip: f32,
        height: f32,
        opacity: f32,
    },
    /// Closing (M9): `clip` tall, `top` below its 0 pt slot, the list
    /// `height` tall under an occluding layer.
    Closing { top: f32, clip: f32, height: f32 },
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
            chevrons: self.chevrons.drawn(),
        }
    }

    /// The viewport's top file changed: a new panel's file opens that panel
    /// (a crossing, OQ-44), and the file is marked.
    pub(super) fn visible_file_changed(&mut self, idx: u32, cx: &mut Context<Self>) {
        // A refresh or an iteration switch: `idx` is a file of the new diff,
        // which the panels do not hold yet. Its partition decides
        // (`FileTree::build`).
        if !Arc::ptr_eq(&self.diff, self.viewport.read(cx).provider()) {
            return;
        }
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

    /// When another panel opened since the last frame (a crossing, a
    /// repartition), its list takes the keyboard from the list that had it,
    /// which is no longer rendered: gpui-kit's `Tree` key context inside
    /// [`KEY_CONTEXT`] in that frame (only the active tab renders, and
    /// activating a tab moves the keyboard into it). The filter box is
    /// inside [`KEY_CONTEXT`] but not `Tree`, so typing there keeps the
    /// keyboard.
    pub(super) fn follow_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.panels.open_key();
        if open == self.rendered_open.as_ref() {
            return;
        }
        // Before a list was rendered, none had the keyboard.
        if self.rendered_open.is_some() {
            let stack = window.context_stack();
            // "Tree" is gpui-base's `tree::CONTEXT` (its `key_context()` is
            // `#[doc(hidden)]`), the context the keymap's `Tree` bindings
            // name: check it when bumping gpui-kit.
            if [KEY_CONTEXT, "Tree"]
                .iter()
                .all(|name| stack.iter().any(|c| c.contains(name)))
            {
                self.focus(window, cx);
            }
        }
        self.rendered_open = open.cloned();
    }

    /// A user opens panel `key` ([`FilesPanel::open_by`]; a header click is
    /// [`Initiator::Pointer`]): its list takes the keyboard and keeps its
    /// scroll position, the diff does not move, and the current file's row
    /// is marked when the panel holds it.
    pub fn open_panel_by(
        &mut self,
        key: &PanelKey,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panels.open_by(key, initiator, window, cx) {
            self.selected = None;
            self.mark_current(false, cx);
            cx.notify();
        }
    }

    /// The accordion for this frame: each shown panel's header, the open
    /// panel's body after its header and, while a switch runs, the closing
    /// body after its own (M9); or why nothing shows.
    pub(super) fn accordion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let swap = self.panels.sample_swap(window, cx);
        if swap.is_some() {
            self.register_swap(window, cx);
        }
        let headers = self.panels.has_headers();
        let filtering = self.filter.is_active();
        let this = cx.entity().downgrade();
        let mut out = Vec::new();
        let mut first = true;
        for (ix, panel) in self.panels.panels().iter().enumerate() {
            if !panel.is_shown() {
                continue;
            }
            let is_open = self.panels.open == Some(ix);
            let offset = swap.map_or(0.0, |s| s.offset(ix));
            if headers {
                let open =
                    swap.and_then(|s| s.chevron(ix))
                        .unwrap_or(if is_open { 1.0 } else { 0.0 });
                let header = header(panel, open, filtering, this.clone(), first, cx);
                out.push(wrap::slide(point(px(0.), px(offset)), header));
                first = false;
            }
            let layer = match swap {
                None if is_open => Some(Layer::Rest),
                Some(s) if ix == s.opening => Some(Layer::Opening {
                    top: offset,
                    clip: s.open,
                    height: s.height,
                    opacity: s.opacity,
                }),
                Some(s) if ix == s.closing && s.closing_clip() > 0.0 => Some(Layer::Closing {
                    top: offset,
                    clip: s.closing_clip(),
                    height: s.height,
                }),
                _ => None,
            };
            if let Some(layer) = layer {
                out.push(self.body(panel, layer, cx));
            }
        }
        if !self.panels.open().is_some_and(PanelTree::is_shown) {
            out.push(self.nothing_shows(cx));
        }
        out
    }

    /// While a switch runs, registers it with `motion::settle`: a mouse
    /// down freezes it, a settling input (or the mouse up after a freeze)
    /// ends it.
    fn register_swap(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (freeze, settle) = (cx.weak_entity(), cx.weak_entity());
        motion::settle::register(
            None,
            move |now, cx| {
                freeze
                    .update(cx, |t, cx| {
                        t.panels.freeze(now);
                        cx.notify();
                    })
                    .ok();
            },
            move |which, cx| {
                settle
                    .update(cx, |t, cx| {
                        t.panels.settle(which);
                        cx.notify();
                    })
                    .ok();
            },
            window,
            cx,
        );
    }

    /// Panel `panel`'s body as `layer` places it: its slot (the open panel's
    /// takes the free height and measures it as H; a closing one's is 0 pt
    /// tall, under its header), in it the clip (`tree-panel-body-{key}`),
    /// and in that the list, at the clip's top.
    fn body(&self, panel: &PanelTree, layer: Layer, cx: &Context<Self>) -> AnyElement {
        let ctx = row::RowCtx::shared(self.row_ctx(panel, cx));
        let selector = format!("tree-panel-body-{}", panel.key);
        let (top, list_height, opacity) = match layer {
            Layer::Rest => (0.0, None, 1.0),
            Layer::Opening {
                top,
                height,
                opacity,
                ..
            } => (top, Some(height), opacity),
            Layer::Closing { top, height, .. } => (top, Some(height), 1.0),
        };
        motion::report(
            || Recorded {
                id: SharedString::from(selector.clone()).into(),
                offset: point(px(0.), px(top)),
                opacity,
                rotation: 0.0,
            },
            cx,
        );
        let list = div()
            .w_full()
            .px(px(edge::SIDEBAR))
            .map(|el| match list_height {
                Some(h) => el.h(px(h)),
                None => el.h_full(),
            })
            .when(opacity < 1.0, |el| el.opacity(opacity))
            .child(
                tree(
                    panel.tree_state(),
                    move |ix, entry, selected, window, cx| {
                        row::render(&ctx, ix, entry, selected, window, cx)
                    },
                )
                .size_full(),
            );
        let clip = div()
            .debug_selector(move || selector)
            .absolute()
            .left_0()
            .right_0()
            .top(px(top))
            .overflow_hidden()
            .map(|el| match layer {
                Layer::Rest => el.bottom_0(),
                Layer::Opening { clip, .. } | Layer::Closing { clip, .. } => el.h(px(clip)),
            })
            .child(list);
        if let Layer::Closing { .. } = layer {
            // Live and inert: the occluding layer takes every click.
            return div()
                .relative()
                .flex_none()
                .w_full()
                .h(px(0.))
                .child(clip.child(div().absolute().inset_0().occlude()))
                .into_any_element();
        }
        let this = cx.entity().downgrade();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        this.update(cx, |t, _| {
                            t.panels.body_height = bounds.size.height.as_f32()
                        })
                        .ok();
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(clip)
            .into_any_element()
    }

    /// Why no panel shows: no files, or none the filters keep.
    fn nothing_shows(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let none = self.files.is_empty();
        let this = cx.entity().downgrade();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(gap::CONTROLS))
            .text_style(text::UI)
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
                        .rounded(px(radius::XS))
                        .outline()
                        .on_click(move |_, window, cx| {
                            if let Some(t) = this.upgrade() {
                                t.update(cx, |t, cx| t.clear_filters(window, cx));
                            }
                        }),
                )
            })
            .into_any_element()
    }
}
