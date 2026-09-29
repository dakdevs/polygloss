//! The review tab's three resizable panes (design §11.1): file tree |
//! diff viewport | threads panel (toggleable). The tree and the panel are
//! T3.6's and T3.9's; until they fill them in, the tree pane lists the files
//! and the panel shows an empty state.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, ResizableState, StyledExt as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px, uniform_list,
};
use polygloss_diff::{FileChange, FileStatus};
use polygloss_viewport::{DiffViewport, ScrollTarget};

use crate::review_tab::ReviewTab;

gpui_kit::actions!(
    tab,
    [
        /// Show or hide the threads panel.
        ToggleThreadsPanel,
    ]
);

/// Initial width of the file tree pane.
const TREE_WIDTH: f32 = 280.0;
/// Initial width of the threads panel.
const THREADS_WIDTH: f32 = 340.0;

/// Pane state of one review tab.
pub(crate) struct Panes {
    pub threads_visible: bool,
    /// Pane widths (kept while the tab lives).
    pub state: Entity<ResizableState>,
}

impl Panes {
    pub fn new(cx: &mut Context<ReviewTab>) -> Panes {
        Panes {
            threads_visible: true,
            state: cx.new(|_| ResizableState::default()),
        }
    }
}

/// The panes of `tab`.
pub(crate) fn render(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let tree = crate::tree::render_pane(tab, window, cx)
        .unwrap_or_else(|| file_list(tab, cx).into_any_element());
    let threads = tab.panes.threads_visible.then(|| {
        crate::threads::render_panel(tab, window, cx)
            .unwrap_or_else(|| empty_threads(cx).into_any_element())
    });
    let border = cx.theme().border;
    let focus = tab.viewport_focus().clone();
    h_resizable("review-panes")
        .with_state(&tab.panes.state)
        .child(
            resizable_panel()
                .size(px(TREE_WIDTH))
                .size_range(px(160.)..px(640.))
                .flex_none()
                .child(
                    div()
                        .debug_selector(|| "file-tree-pane".into())
                        .size_full()
                        .border_r_1()
                        .border_color(border)
                        .child(tree),
                ),
        )
        .child(
            resizable_panel().size_range(px(320.)..px(100_000.)).child(
                div()
                    .debug_selector(|| "viewport-pane".into())
                    .key_context("Viewport")
                    .track_focus(tab.viewport_focus())
                    // A click anywhere in the diff gives it the keyboard.
                    .capture_any_mouse_down(move |_, window, cx| window.focus(&focus, cx))
                    .size_full()
                    .overflow_hidden()
                    .child(tab.viewport.clone()),
            ),
        )
        .child(
            resizable_panel()
                .size(px(THREADS_WIDTH))
                .size_range(px(220.)..px(720.))
                .flex_none()
                .visible(threads.is_some())
                .when_some(threads, |panel, threads| {
                    panel.child(
                        div()
                            .debug_selector(|| "threads-pane".into())
                            .size_full()
                            .border_l_1()
                            .border_color(border)
                            .child(threads),
                    )
                }),
        )
        .into_any_element()
}

/// A pane's title row.
fn pane_header(
    title: SharedString,
    detail: Option<SharedString>,
    cx: &Context<ReviewTab>,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .h(px(32.))
        .px_3()
        .gap_2()
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .font_semibold()
        .text_color(theme.muted_foreground)
        .child(title)
        .when_some(detail, |row, d| row.child(div().font_normal().child(d)))
}

/// Status letter of a change, as the tree shows it.
pub fn status_letter(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "A",
        FileStatus::Modified => "M",
        FileStatus::Deleted => "D",
        FileStatus::Renamed => "R",
        FileStatus::TypeChanged => "T",
    }
}

/// The plain file list shown until the file tree (T3.6) exists: status
/// letter and path per file; clicking scrolls the viewport to it.
fn file_list(tab: &ReviewTab, cx: &Context<ReviewTab>) -> impl IntoElement {
    let files: Arc<Vec<FileChange>> = tab.opened.files.clone();
    let viewport: Entity<DiffViewport> = tab.viewport.clone();
    let theme = cx.theme().clone();
    let count = files.len();
    v_flex()
        .size_full()
        .bg(theme.sidebar)
        .child(pane_header(
            "FILES".into(),
            Some(count.to_string().into()),
            cx,
        ))
        .child(
            uniform_list("file-list", count, move |range, _window, _cx| {
                range
                    .map(|i| {
                        let f = &files[i];
                        let color = match f.status {
                            FileStatus::Added => theme.green,
                            FileStatus::Deleted => theme.red,
                            FileStatus::Renamed => theme.blue,
                            _ => theme.yellow,
                        };
                        let viewport = viewport.clone();
                        h_flex()
                            .id(("file", i))
                            .h(px(26.))
                            .px_3()
                            .gap_2()
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .child(
                                div()
                                    .w(px(12.))
                                    .flex_none()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(color)
                                    .child(status_letter(f.status)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_color(theme.foreground)
                                    .child(f.display_path().to_string()),
                            )
                            .on_click(move |_, _, cx| {
                                viewport.update(cx, |v, cx| {
                                    v.scroll_to(ScrollTarget::File(i as u32), cx)
                                });
                            })
                    })
                    .collect()
            })
            .flex_1(),
        )
}

/// The threads panel until T3.9 fills it.
fn empty_threads(cx: &Context<ReviewTab>) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .bg(theme.sidebar)
        .child(pane_header("THREADS".into(), None, cx))
        .child(
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No threads yet")
                .child(div().text_xs().child("Press C on a line to comment.")),
        )
}
