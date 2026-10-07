//! One row of the file tree (design §11.5): chevron, the outline `folder` or
//! `file` icon, name, then right-aligned the "changed since viewed" dot,
//! open-thread pill, agent icon, `+a −d` and the status letter, and last the
//! Viewed slot (OQ-43): an empty `circle` while the row is hovered,
//! `circle-minus` on a partly viewed folder, `circle-check` once viewed. The
//! slot's mouse-down stops at a wrapper, so it never selects the row or folds
//! a folder; the icon is never a toggle.
//!
//! Columns (ADR-0031 tree rows), from the row highlight's edge (itself
//! `edge::SIDEBAR` in): at depth n the chevron's `ICON_XS` box at
//! `ICON_LEAD` plus n · `TREE_INDENT`, the `ICON_SM` icon `gap::INLINE`
//! after it, the label `gap::ICON_LABEL` after that; a file keeps the
//! chevron's slot empty. The Viewed circle's icon ends `ICON_LEAD` from the
//! highlight's end, as leading icons start.
//!
//! A folder's chevron turns when the pointer toggles it (M10, ADR-0030):
//! 0° (open, pointing down) ↔ −90° over MICRO on MOVE, its rows inserted or
//! gone on the commit frame; the arrows (the tree's only folder keys),
//! reveals and restores put it at its end. The turns are [`Chevrons`], kept
//! by the `FileTree`.
//!
//! Debug selectors, `key` being the row's item id (`f:3`, `d:src`):
//! `tree-row-{key}`, `tree-chevron-{key}` (the chevron's slot),
//! `tree-icon-{key}: {icon}`, `tree-name-{key}`, the slot
//! `tree-check-{key}` and its glyph `tree-slot-{key}: {glyph}` (painted only
//! while it shows), `tree-stats-{file}: +a −d`, `tree-status-{file}: {letter}`.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::TreeEntry;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, Entity, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity,
    Window, div, px, radians,
};
use polygloss_diff::{FileChange, FileStatus};
use polygloss_viewport::{DiffViewport, FileCounts, FileFlags, group_digits};

use super::model::{ItemId, TreeModel};
use super::panels::PanelKey;
use super::{FileTree, FileTreeEvent};
use crate::motion::ink::PressInk as _;
use crate::motion::{
    self, Initiator, Motion, MotionPolicy, Recorded, Reduced, ReducedPlay, Settle, Track, tokens,
};
use crate::review_tab::toolbar::tooltip;
use crate::space::{TextStyleExt as _, gap, height, layout, pad, radius, size, text};

/// The hover group of a row: its empty Viewed circle shows while it is
/// hovered.
const ROW_GROUP: &str = "tree-row";

/// A folder chevron's turn (ADR-0030 M10): MICRO both ways on MOVE, by
/// pointer only; under Reduce Motion it jumps.
pub const CHEVRON: Motion = Motion {
    enter: tokens::MICRO,
    exit: tokens::MICRO,
    easing: tokens::in_out,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Snap,
        exit: ReducedPlay::Snap,
    },
};

/// The angle of a chevron that is `open` (0 → 1), in degrees: 0° open,
/// pointing down; −90° closed.
fn chevron_angle(open: f32) -> f32 {
    tokens::CHEVRON_CLOSED_DEG * (1.0 - open)
}

/// The folder chevrons turning in a tree (M10), kept by its `FileTree`
/// (rule 13): one [`Track`] per turning folder, 1 open and 0 closed.
#[derive(Default)]
pub struct Chevrons {
    /// Each turning folder's track and whether it turns open.
    turning: HashMap<(PanelKey, SharedString), (Track, bool)>,
    /// The folder row a left mouse down landed on: the toggle it causes is
    /// the pointer's.
    pressed: Option<SharedString>,
    /// The open panel's turning chevrons as last sampled: the angle and
    /// whether each turns open.
    drawn: Arc<HashMap<SharedString, (f32, bool)>>,
}

impl Chevrons {
    /// A left mouse down on folder row `key` (before the tree toggles it).
    pub fn press(&mut self, key: SharedString) {
        self.pressed = Some(key);
    }

    /// Forgets a press that toggled nothing.
    fn forget_press(&mut self) {
        self.pressed = None;
    }

    /// Folder row `key` of `panel` was expanded (`open`) or collapsed: after
    /// a press on its row the chevron turns there from where it is painted
    /// (a turn under way retargets); any other toggle (the arrows, a reveal,
    /// a restore) puts it there at once.
    pub fn toggled(
        &mut self,
        panel: &PanelKey,
        key: SharedString,
        open: bool,
        policy: MotionPolicy,
        now: Instant,
    ) {
        let pressed = self.pressed.take().is_some_and(|p| p == key);
        let slot = (panel.clone(), key);
        if !pressed {
            self.turning.remove(&slot);
            return;
        }
        let (to, from) = if open { (1.0, 0.0) } else { (0.0, 1.0) };
        let (track, opens) = self
            .turning
            .entry(slot)
            .or_insert_with(|| (Track::new(from, 1.0), open));
        *opens = open;
        track.retarget(to, &CHEVRON, Initiator::Pointer, policy, now);
    }

    /// Samples the chevrons of `panel` (the open one) for this frame,
    /// requesting frames while one turns; those that settled or belong to
    /// another panel are dropped. Call it from the tree's render.
    fn sample(&mut self, panel: Option<&PanelKey>, window: &mut Window, cx: &App) {
        self.turning
            .retain(|(p, _), (track, _)| Some(p) == panel && !track.is_settled());
        let mut drawn = HashMap::with_capacity(self.turning.len());
        for ((_, key), (track, opens)) in &mut self.turning {
            let sample = motion::sample(track, window, cx);
            drawn.insert(key.clone(), (chevron_angle(sample.value), *opens));
        }
        self.drawn = Arc::new(drawn);
    }

    /// `tree`'s frame of its chevrons (from its render): forgets a press
    /// that toggled nothing, samples the open panel's turns and, while one
    /// runs, registers them with `motion::settle`.
    pub fn frame(
        tree: &mut FileTree,
        open: Option<&PanelKey>,
        window: &mut Window,
        cx: &mut Context<FileTree>,
    ) {
        tree.chevrons.forget_press();
        tree.chevrons.sample(open, window, cx);
        if !tree.chevrons.is_turning() {
            return;
        }
        let (freeze, settle) = (cx.weak_entity(), cx.weak_entity());
        motion::settle::register(
            None,
            move |now, cx| {
                freeze
                    .update(cx, |t, cx| {
                        t.chevrons.freeze(now);
                        cx.notify();
                    })
                    .ok();
            },
            move |which, cx| {
                settle
                    .update(cx, |t, cx| {
                        t.chevrons.settle(which);
                        cx.notify();
                    })
                    .ok();
            },
            window,
            cx,
        );
    }

    /// What [`Self::sample`] drew, for the rows.
    pub fn drawn(&self) -> Arc<HashMap<SharedString, (f32, bool)>> {
        self.drawn.clone()
    }

    fn is_turning(&self) -> bool {
        !self.turning.is_empty()
    }

    /// A mouse down: every turn holds where it is.
    fn freeze(&mut self, now: Instant) {
        for (track, _) in self.turning.values_mut() {
            track.freeze(now);
        }
    }

    /// A settling input: every turn (`Settle::All`) or those still frozen
    /// ends now.
    fn settle(&mut self, which: Settle) {
        self.turning
            .retain(|_, (track, _)| which == Settle::Frozen && !track.is_frozen());
    }
}

/// What rows read while the tree renders them.
pub struct RowCtx {
    pub files: Arc<Vec<FileChange>>,
    pub flags: Arc<Vec<FileFlags>>,
    pub dir_viewed: Arc<HashMap<String, (u32, u32)>>,
    pub model: Arc<TreeModel>,
    pub viewport: Entity<DiffViewport>,
    pub tree: WeakEntity<FileTree>,
    pub status: StatusColors,
    /// `+a` and `−d` (the viewport's `stat_added` / `stat_removed`).
    pub stat_added: Hsla,
    pub stat_removed: Hsla,
    /// The turning chevrons ([`Chevrons::drawn`]).
    pub chevrons: Arc<HashMap<SharedString, (f32, bool)>>,
}

impl RowCtx {
    pub fn shared(self) -> Rc<RowCtx> {
        Rc::new(self)
    }
}

/// The status letters' colors: the theme's `version_control.*` keys,
/// else gpui-kit's base colors.
#[derive(Debug, Clone, Copy)]
pub struct StatusColors {
    added: Hsla,
    modified: Hsla,
    deleted: Hsla,
    renamed: Hsla,
}

impl StatusColors {
    /// The active theme's.
    pub fn of(cx: &App) -> StatusColors {
        let kit = cx.theme();
        let zed = cx
            .try_global::<crate::theme::ActiveTheme>()
            .map(|t| t.zed.clone());
        let pick = |key: &str, fallback: Hsla| -> Hsla {
            zed.as_ref()
                .and_then(|t| t.color(key))
                .map_or(fallback, |c| gpui_kit::rgba(c.to_u32()).into())
        };
        StatusColors {
            added: pick("version_control.added", kit.green),
            modified: pick("version_control.modified", kit.yellow),
            deleted: pick("version_control.deleted", kit.red),
            renamed: pick("version_control.renamed", kit.blue),
        }
    }

    /// The status letter's text and color (a type change reads as
    /// modified).
    pub fn badge(&self, status: FileStatus) -> (&'static str, Hsla) {
        match status {
            FileStatus::Added => ("A", self.added),
            FileStatus::Modified => ("M", self.modified),
            FileStatus::Deleted => ("D", self.deleted),
            FileStatus::Renamed => ("R", self.renamed),
            FileStatus::TypeChanged => ("T", self.modified),
        }
    }
}

/// The status letter's text and color in the active theme.
pub fn status_badge(status: FileStatus, cx: &App) -> (&'static str, Hsla) {
    StatusColors::of(cx).badge(status)
}

/// A Viewed state: not viewed, viewed, or some files of a folder viewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    Mixed,
}

impl Check {
    /// A folder's state with `viewed` of its `total` files viewed.
    pub fn of(viewed: u32, total: u32) -> Check {
        match viewed {
            0 => Check::Off,
            v if v >= total => Check::On,
            _ => Check::Mixed,
        }
    }
}

/// The row of `entry`.
pub fn render(
    ctx: &Rc<RowCtx>,
    _ix: usize,
    entry: &TreeEntry,
    _selected: bool,
    _window: &mut Window,
    cx: &mut App,
) -> ListItem {
    let item = entry.item();
    let key = item.id.clone();
    let base = ListItem::new(SharedString::from(format!("tree-row-{key}")))
        .debug_selector({
            let key = key.clone();
            move || format!("tree-row-{key}")
        })
        .group(ROW_GROUP)
        .h(px(height::MD))
        .py_0()
        .pl(px(pad::ICON_LEAD + size::TREE_INDENT * entry.depth() as f32))
        // The slot is wider than its icon, which ends `ICON_LEAD` in.
        .pr(px(
            pad::ICON_LEAD - (layout::VIEWED_SLOT - size::ICON_SM) / 2.0
        ))
        .text_style(text::UI)
        .rounded(px(radius::for_height(height::MD)));
    match ItemId::parse(&key) {
        Some(ItemId::Dir(path)) => folder_row(ctx, base, entry, &key, path, cx),
        Some(ItemId::File(idx)) => file_row(ctx, base, entry, &key, idx, cx),
        None => base.child(item.label.clone()),
    }
}

fn folder_row(
    ctx: &RowCtx,
    base: ListItem,
    entry: &TreeEntry,
    key: &SharedString,
    path: String,
    cx: &App,
) -> ListItem {
    let (viewed, total) = ctx.dir_viewed.get(&path).copied().unwrap_or((0, 0));
    let check = Check::of(viewed, total);
    // The folder's files are read on click (a root folder of a large diff
    // lists thousands).
    let (model, tree) = (ctx.model.clone(), ctx.tree.clone());
    let toggle = move |cx: &mut App| {
        let files = model.dir(&path).map(|n| n.files.clone());
        let event = FileTreeEvent::ToggleFolderViewed {
            dir: path.clone(),
            files: files.unwrap_or_default(),
        };
        if let Some(tree) = tree.upgrade() {
            tree.update(cx, |_, cx| cx.emit(event));
        }
    };
    let label = entry.item().label.clone();
    // The press comes before the tree's own mouse down toggles the folder:
    // that toggle is the pointer's (M10).
    let (pressed, press_key) = (ctx.tree.clone(), key.clone());
    base.accessibility_label(label.clone())
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            if let Some(tree) = pressed.upgrade() {
                tree.update(cx, |t, _| t.chevrons.press(press_key.clone()));
            }
        })
        .child(
            h_flex()
                .w_full()
                .gap(px(gap::ICON_LABEL))
                .child(lead(ctx, key, entry, Lucide::Folder, cx))
                .child(name(key, label, check == Check::On, false, cx))
                .child(slot(key, check, cx, toggle)),
        )
}

fn file_row(
    ctx: &RowCtx,
    base: ListItem,
    entry: &TreeEntry,
    key: &SharedString,
    idx: u32,
    cx: &App,
) -> ListItem {
    let Some(file) = ctx.files.get(idx as usize) else {
        return base;
    };
    let flags = ctx.flags.get(idx as usize).copied().unwrap_or_default();
    let counts = ctx.viewport.read(cx).file_counts(idx);
    let (tree, click_tree) = (ctx.tree.clone(), ctx.tree.clone());
    let toggle = move |cx: &mut App| {
        if let Some(tree) = tree.upgrade() {
            tree.update(cx, |_, cx| cx.emit(FileTreeEvent::ToggleViewed(idx)));
        }
    };
    let check = if flags.viewed { Check::On } else { Check::Off };
    let deleted = file.status == FileStatus::Deleted;
    base.accessibility_label(SharedString::from(file.display_path().to_owned()))
        .on_click(move |_, _, cx| {
            if let Some(tree) = click_tree.upgrade() {
                tree.update(cx, |t, cx| t.select_file(idx, cx));
            }
        })
        .child(
            h_flex()
                .w_full()
                .gap(px(gap::ICON_LABEL))
                .child(lead(ctx, key, entry, Lucide::File, cx))
                .child(name(
                    key,
                    entry.item().label.clone(),
                    flags.viewed,
                    deleted,
                    cx,
                ))
                .child(badges(ctx, idx, file.status, flags, counts, cx))
                .child(slot(key, check, cx, toggle)),
        )
}

/// The chevron's slot, then `gap::INLINE` later the row's `icon`.
fn lead(
    ctx: &RowCtx,
    key: &SharedString,
    entry: &TreeEntry,
    glyph: Lucide,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap(px(gap::INLINE))
        .child(chevron(ctx, key, entry, cx))
        .child(icon(key, glyph, cx))
}

/// A folder's expand chevron, one glyph turned about its centre (M10); an
/// empty slot of the same width for files.
fn chevron(ctx: &RowCtx, key: &SharedString, entry: &TreeEntry, cx: &App) -> impl IntoElement {
    let selector = format!("tree-chevron-{key}");
    let angle = entry.is_folder().then(|| {
        let open = entry.is_expanded();
        // A turn toward the state the row shows; at rest, that state.
        let angle = match ctx.chevrons.get(key) {
            Some(&(angle, opens)) if opens == open => angle,
            _ => chevron_angle(if open { 1.0 } else { 0.0 }),
        };
        motion::report(
            || Recorded {
                id: SharedString::from(format!("tree-chevron-{key}")).into(),
                offset: Default::default(),
                opacity: 1.0,
                rotation: angle,
            },
            cx,
        );
        angle
    });
    div()
        .debug_selector(move || selector)
        .w(px(size::ICON_XS))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .when_some(angle, |el, angle| {
            el.child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .rotate(radians(angle.to_radians()))
                    .text_color(cx.theme().muted_foreground),
            )
        })
}

/// `icon`'s Lucide name (`folder`, `circle-check`): its file name in
/// `icons/<name>.svg`, so the debug selectors name what is drawn.
fn glyph_name(icon: Lucide) -> String {
    let path = icon.path();
    path.trim_start_matches("icons/")
        .trim_end_matches(".svg")
        .to_owned()
}

/// The outline icon before the name.
fn icon(key: &SharedString, icon: Lucide, cx: &App) -> impl IntoElement {
    let selector = format!("tree-icon-{key}: {}", glyph_name(icon));
    div()
        .debug_selector(move || selector)
        .flex_none()
        .flex()
        .child(
            Icon::new(icon)
                .with_size(px(size::ICON_SM))
                .text_color(cx.theme().muted_foreground),
        )
}

/// The name: dimmed when viewed, struck through when deleted.
fn name(
    key: &SharedString,
    label: SharedString,
    viewed: bool,
    deleted: bool,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let selector = format!("tree-name-{key}");
    div()
        .debug_selector(move || selector)
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(if viewed {
            theme.muted_foreground
        } else {
            theme.foreground
        })
        .when(deleted, |el| el.line_through())
        .child(label)
}

/// A file's right-aligned badges: the "changed since viewed" dot, open
/// threads, the agent icon, `+a −d` once counted and the status letter.
fn badges(
    ctx: &RowCtx,
    idx: u32,
    status: FileStatus,
    flags: FileFlags,
    counts: Option<FileCounts>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let (letter, color) = ctx.status.badge(status);
    h_flex()
        .flex_none()
        .gap(px(gap::ICON_LABEL))
        .text_style(text::SMALL)
        .when(flags.changed_since_viewed, |el| {
            el.child(
                div()
                    .debug_selector(move || format!("tree-changed-{idx}"))
                    .id(SharedString::from(format!("tree-changed-{idx}")))
                    .size(px(size::DOT))
                    .rounded_full()
                    .bg(theme.blue)
                    .tooltip(tooltip("Changed since viewed")),
            )
        })
        .when(flags.open_threads > 0, |el| {
            let n = flags.open_threads;
            el.child(
                div()
                    .id(SharedString::from(format!("tree-threads-{idx}")))
                    .debug_selector(move || format!("tree-threads-{idx}"))
                    .px(px(pad::BADGE_X))
                    .rounded_full()
                    .font_features(crate::chrome::tabular_figures())
                    .bg(theme.muted)
                    .text_color(theme.foreground)
                    .child(n.to_string())
                    .tooltip(tooltip(match n {
                        1 => "1 open thread".to_owned(),
                        n => format!("{n} open threads"),
                    })),
            )
        })
        .when(flags.agent_threads, |el| {
            el.child(
                div()
                    .debug_selector(move || format!("tree-agent-{idx}"))
                    .text_color(theme.primary)
                    .child(Icon::new(IconName::Bot).xsmall()),
            )
        })
        .when_some(counts, |el, c| el.child(stats(ctx, idx, c, cx)))
        .child(
            div()
                .debug_selector(move || format!("tree-status-{idx}: {letter}"))
                .w(px(size::STATUS_COL))
                .flex()
                .justify_center()
                .font_semibold()
                .text_color(color)
                .child(letter),
        )
}

/// `+a −d` in the code font, both shown (zeros too), grouped by thousands.
fn stats(ctx: &RowCtx, idx: u32, c: FileCounts, cx: &App) -> impl IntoElement {
    let added = format!("+{}", group_digits(c.additions.into()));
    let removed = format!("−{}", group_digits(c.deletions.into()));
    let selector = format!("tree-stats-{idx}: {added} {removed}");
    h_flex()
        .debug_selector(move || selector)
        .gap(px(gap::INLINE))
        .text_style(text::CODE_CHROME)
        .font_family(cx.theme().mono_font_family.clone())
        .font_medium()
        .child(div().text_color(ctx.stat_added).child(added))
        .child(div().text_color(ctx.stat_removed).child(removed))
}

/// The Viewed slot at the row's end: `circle-check` once viewed,
/// `circle-minus` on a partly viewed folder, else an empty `circle` shown
/// only while the row is hovered. The outer div stops the mouse-down, so the
/// tree neither selects the row nor folds the folder; the inner one takes
/// the click, shows press ink and says what a click does.
fn slot(
    key: &SharedString,
    check: Check,
    cx: &App,
    on_toggle: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let theme = cx.theme();
    let (glyph, color) = match check {
        Check::On => (Lucide::CircleCheck, theme.primary),
        Check::Mixed => (Lucide::CircleMinus, theme.muted_foreground),
        Check::Off => (Lucide::Circle, theme.muted_foreground),
    };
    let glyph_selector = format!("tree-slot-{key}: {}", glyph_name(glyph));
    let selector = format!("tree-check-{key}");
    div()
        .debug_selector(move || selector)
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id(SharedString::from(format!("tree-check-{key}")))
                .size(px(layout::VIEWED_SLOT))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .press_ink(cx)
                .tooltip(tooltip(if check == Check::On {
                    "Mark unviewed (v)"
                } else {
                    "Mark viewed (v)"
                }))
                .on_click(move |_, _, cx| on_toggle(cx))
                .child(
                    div()
                        .when(check == Check::Off, |el| {
                            el.invisible().group_hover(ROW_GROUP, |s| s.visible())
                        })
                        .child(
                            div()
                                .debug_selector(move || glyph_selector)
                                .flex()
                                .child(Icon::new(glyph).small().text_color(color)),
                        ),
                ),
        )
}
