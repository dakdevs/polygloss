//! One row of the file tree (design §11.5): chevron, the outline `folder` or
//! `file` icon, name, then right-aligned the "changed since viewed" dot,
//! open-thread pill, agent icon, `+a −d` and the status letter, and last the
//! Viewed slot (OQ-43): an empty `circle` while the row is hovered,
//! `circle-minus` on a partly viewed folder, `circle-check` once viewed. The
//! slot's mouse-down stops at a wrapper, so it never selects the row or folds
//! a folder; the icon is never a toggle.
//!
//! Debug selectors, `key` being the row's item id (`f:3`, `d:src`):
//! `tree-row-{key}`, `tree-icon-{key}: {icon}`, `tree-name-{key}`, the slot
//! `tree-check-{key}` and its glyph `tree-slot-{key}: {glyph}` (painted only
//! while it shows), `tree-stats-{file}: +a −d`, `tree-status-{file}: {letter}`.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::TreeEntry;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Entity, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, px,
};
use polygloss_diff::{FileChange, FileStatus};
use polygloss_viewport::{DiffViewport, FileCounts, FileFlags, group_digits};

use super::model::{ItemId, TreeModel};
use super::{FileTree, FileTreeEvent};
use crate::review_tab::toolbar::tooltip;

/// Row height (pt, design §11.5).
pub const ROW_HEIGHT: f32 = 28.0;
/// Indentation per level (pt).
const INDENT: f32 = 14.0;
/// The Viewed slot's side (pt).
const SLOT: f32 = 20.0;
/// The hover group of a row: its empty Viewed circle shows while it is
/// hovered.
const ROW_GROUP: &str = "tree-row";

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
        .h(px(ROW_HEIGHT))
        .py_0()
        .pl(px(6. + INDENT * entry.depth() as f32))
        .pr_1()
        .text_sm()
        .rounded(px(6.));
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
    base.accessibility_label(label.clone()).child(
        h_flex()
            .w_full()
            .gap_1p5()
            .child(chevron(entry, cx))
            .child(icon(key, Lucide::Folder, cx))
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
                .gap_1p5()
                .child(chevron(entry, cx))
                .child(icon(key, Lucide::File, cx))
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

/// A folder's expand chevron; an empty column of the same width for files.
fn chevron(entry: &TreeEntry, cx: &App) -> impl IntoElement {
    div()
        .w(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .when(entry.is_folder(), |el| {
            el.child(
                Icon::new(if entry.is_expanded() {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
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
        .child(Icon::new(icon).text_color(cx.theme().muted_foreground))
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
        .gap_1p5()
        .text_xs()
        .when(flags.changed_since_viewed, |el| {
            el.child(
                div()
                    .debug_selector(move || format!("tree-changed-{idx}"))
                    .id(SharedString::from(format!("tree-changed-{idx}")))
                    .size(px(6.))
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
                    .px_1p5()
                    .rounded_full()
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
                .w(px(10.))
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
        .gap_1()
        .font_family(cx.theme().mono_font_family.clone())
        .font_medium()
        .child(div().text_color(ctx.stat_added).child(added))
        .child(div().text_color(ctx.stat_removed).child(removed))
}

/// The Viewed slot at the row's end: `circle-check` once viewed,
/// `circle-minus` on a partly viewed folder, else an empty `circle` shown
/// only while the row is hovered. The outer div stops the mouse-down, so the
/// tree neither selects the row nor folds the folder; the inner one takes
/// the click, highlights under the pointer and says what a click does.
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
                .size(px(SLOT))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.08)))
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
