//! The review tab's three resizable panes (design §11.1): file tree
//! (T3.6's `tree::render_pane`) | diff viewport | threads panel
//! (toggleable; T3.9's, an empty state until it fills it in).

use gpui_kit::component::{
    ActiveTheme as _, ResizableState, StyledExt as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};

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
    // Every review tab gets a file tree (`features::attach`).
    let tree = crate::tree::render_pane(tab, window, cx)
        .unwrap_or_else(|| div().size_full().bg(cx.theme().sidebar).into_any_element());
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
