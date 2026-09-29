//! One row of the file tree (design §11.5): chevron, Viewed checkbox (its
//! mouse-down stops at a wrapper, so it never toggles or selects the row),
//! name, then the "changed since viewed" dot, open-thread and agent badges,
//! +/− counts and the status letter.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::TreeEntry;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Entity, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, px,
};
use polygloss_diff::{FileChange, FileStatus};
use polygloss_viewport::{DiffViewport, FileFlags};

use super::model::{ItemId, TreeModel};
use super::{FileTree, FileTreeEvent};

/// Row height (px).
pub const ROW_HEIGHT: f32 = 26.0;
/// Indentation per level (px).
const INDENT: f32 = 12.0;

/// What rows read while the tree renders them.
pub struct RowCtx {
    pub files: Arc<Vec<FileChange>>,
    pub flags: Arc<Vec<FileFlags>>,
    pub dir_viewed: Arc<HashMap<String, (u32, u32)>>,
    pub model: Arc<TreeModel>,
    pub viewport: Entity<DiffViewport>,
    pub tree: WeakEntity<FileTree>,
}

impl RowCtx {
    pub fn shared(self) -> Rc<RowCtx> {
        Rc::new(self)
    }
}

/// The status letter's text and color.
pub fn status_badge(status: FileStatus, cx: &App) -> (&'static str, Hsla) {
    let theme = cx.theme();
    match status {
        FileStatus::Added => ("A", theme.green),
        FileStatus::Modified => ("M", theme.yellow),
        FileStatus::Deleted => ("D", theme.red),
        FileStatus::Renamed => ("R", theme.blue),
        FileStatus::TypeChanged => ("T", theme.yellow),
    }
}

/// Checkbox look: unchecked, checked, or some files of a folder viewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    Mixed,
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
    let id = ItemId::parse(&key);
    let theme = cx.theme().clone();
    let base = ListItem::new(SharedString::from(format!("tree-row-{key}")))
        .debug_selector({
            let key = key.clone();
            move || format!("tree-row-{key}")
        })
        .h(px(ROW_HEIGHT))
        .py_0()
        .pl(px(6. + INDENT * entry.depth() as f32))
        .pr_2()
        .text_sm()
        .rounded(px(4.));

    let chevron = div()
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
                .text_color(theme.muted_foreground),
            )
        });

    match id {
        Some(ItemId::Dir(path)) => {
            let (viewed, total) = ctx.dir_viewed.get(&path).copied().unwrap_or((0, 0));
            let check = match viewed {
                0 => Check::Off,
                v if v == total => Check::On,
                _ => Check::Mixed,
            };
            let files = ctx
                .model
                .dir(&path)
                .map(|n| n.files.clone())
                .unwrap_or_default();
            let tree = ctx.tree.clone();
            let dir = path.clone();
            base.accessibility_label(item.label.clone()).child(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .child(chevron)
                    .child(checkbox(&key, check, "Mark folder viewed", cx, move |cx| {
                        let event = FileTreeEvent::ToggleFolderViewed {
                            dir: dir.clone(),
                            files: files.clone(),
                        };
                        if let Some(tree) = tree.upgrade() {
                            tree.update(cx, |_, cx| cx.emit(event));
                        }
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(item.label.clone()),
                    ),
            )
        }
        Some(ItemId::File(idx)) => {
            let Some(file) = ctx.files.get(idx as usize) else {
                return base;
            };
            let flags = ctx.flags.get(idx as usize).copied().unwrap_or_default();
            let counts = ctx.viewport.read(cx).file_counts(idx);
            let (letter, color) = status_badge(file.status, cx);
            let tree = ctx.tree.clone();
            let click_tree = ctx.tree.clone();
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
                        .child(chevron)
                        .child(checkbox(
                            &key,
                            if flags.viewed { Check::On } else { Check::Off },
                            "Viewed",
                            cx,
                            move |cx| {
                                if let Some(tree) = tree.upgrade() {
                                    tree.update(cx, |_, cx| {
                                        cx.emit(FileTreeEvent::ToggleViewed(idx))
                                    });
                                }
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if flags.viewed {
                                    theme.muted_foreground
                                } else {
                                    theme.foreground
                                })
                                .when(file.status == FileStatus::Deleted, |el| el.line_through())
                                .child(item.label.clone()),
                        )
                        .child(
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
                                            .tooltip(|window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    "Changed since viewed",
                                                )
                                                .build(window, cx)
                                            }),
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
                                            .tooltip(move |window, cx| {
                                                let text = match n {
                                                    1 => "1 open thread".to_owned(),
                                                    n => format!("{n} open threads"),
                                                };
                                                gpui_kit::component::tooltip::Tooltip::new(text)
                                                    .build(window, cx)
                                            }),
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
                                .when_some(counts, |el, c| {
                                    el.child(
                                        h_flex()
                                            .gap_1()
                                            .when(c.additions > 0, |el| {
                                                el.child(
                                                    div()
                                                        .text_color(theme.green)
                                                        .child(format!("+{}", c.additions)),
                                                )
                                            })
                                            .when(c.deletions > 0, |el| {
                                                el.child(
                                                    div()
                                                        .text_color(theme.red)
                                                        .child(format!("−{}", c.deletions)),
                                                )
                                            }),
                                    )
                                })
                                .child(
                                    div()
                                        .w(px(10.))
                                        .flex()
                                        .justify_center()
                                        .font_semibold()
                                        .text_color(color)
                                        .child(letter),
                                ),
                        ),
                )
        }
        None => base.child(item.label.clone()),
    }
}

/// A 14 px checkbox. The outer div stops the mouse-down, so the tree
/// neither selects the row nor toggles a folder; the inner one handles the
/// click.
fn checkbox(
    key: &SharedString,
    check: Check,
    tooltip: &'static str,
    cx: &App,
    on_toggle: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let theme = cx.theme();
    let filled = check != Check::Off;
    let selector = format!("tree-check-{key}");
    div()
        .debug_selector(move || selector.clone())
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id(SharedString::from(format!("tree-check-{key}")))
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .map(|el| {
                    if filled {
                        el.bg(theme.primary).border_color(theme.primary)
                    } else {
                        el.bg(theme.background).border_color(theme.input)
                    }
                })
                .cursor_pointer()
                .when(check == Check::On, |el| {
                    el.child(
                        Icon::new(IconName::Check)
                            .xsmall()
                            .text_color(theme.primary_foreground),
                    )
                })
                .when(check == Check::Mixed, |el| {
                    el.child(
                        Icon::new(IconName::Minus)
                            .xsmall()
                            .text_color(theme.primary_foreground),
                    )
                })
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip).build(window, cx)
                })
                .on_click(move |_, _, cx| on_toggle(cx)),
        )
}
