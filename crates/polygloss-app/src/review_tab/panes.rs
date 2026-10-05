//! The review tab's panes (design §11.1): the sidebar's content (T3.6's
//! `tree::render_pane`, find in its place, or the Reviews list) and the main
//! column's resizable diff viewport | threads panel (toggleable; T3.9's).

use gpui_kit::component::{
    ActiveTheme as _, ResizableState, StyledExt as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};

use crate::chrome::{self, Segment};
use crate::keyboard::Pane;
use crate::review_tab::ReviewTab;

gpui_kit::actions!(
    tab,
    [
        /// Show or hide the threads panel.
        ToggleThreadsPanel,
    ]
);

/// Initial width of the threads panel.
const THREADS_WIDTH: f32 = 340.0;

/// Pane state of one review tab.
pub(crate) struct Panes {
    pub threads_visible: bool,
    /// The viewport | threads panel widths (kept while the tab lives).
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

/// The pane of `tab` that shows a focus ring: the one with the keyboard
/// while the keyboard is in use, like macOS's and the web's focus-visible
/// (a click does not light it up; a composer draws its own).
fn ringed_pane(tab: &ReviewTab, window: &Window, cx: &Context<ReviewTab>) -> Option<Pane> {
    window
        .last_input_was_keyboard()
        .then(|| crate::keyboard::focused_pane(tab, window, cx))
        .flatten()
}

/// The sidebar of `tab`: its top row, then the window's segment: the file
/// tree (find, T3.15, in its place while open) or the Reviews list.
pub(crate) fn render_sidebar(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let top_row = chrome::sidebar_top_row(true, window, cx);
    let segment = chrome::chrome(cx).read(cx).segment();
    let content = match segment {
        Segment::Reviews => crate::home::nav::render_nav(window, cx),
        Segment::Files => {
            // Every review tab gets a file tree (`features::attach`).
            let tree = crate::find::render_pane(tab, window, cx)
                .or_else(|| crate::tree::render_pane(tab, window, cx))
                .unwrap_or_else(|| div().size_full().into_any_element());
            let ring = (ringed_pane(tab, window, cx) == Some(Pane::Tree))
                .then(|| focus_ring("focus-ring-tree", cx));
            div()
                .debug_selector(|| "file-tree-pane".into())
                .relative()
                .size_full()
                .child(tree)
                .children(ring)
                .into_any_element()
        }
    };
    v_flex()
        .size_full()
        .child(top_row)
        .child(div().flex_1().min_h_0().child(content))
        .into_any_element()
}

/// The main column's panes of `tab`: diff viewport | threads panel.
pub(crate) fn render(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let threads = tab.panes.threads_visible.then(|| {
        crate::threads::render_panel(tab, window, cx)
            .unwrap_or_else(|| empty_threads(cx).into_any_element())
    });
    let border = cx.theme().border;
    let focus = tab.viewport_focus().clone();
    let focused = ringed_pane(tab, window, cx);
    let ring = |pane: Pane, selector: &'static str| {
        (focused.as_ref() == Some(&pane)).then(|| focus_ring(selector, cx))
    };
    let (viewport_ring, threads_ring) = (
        ring(Pane::Viewport, "focus-ring-viewport"),
        ring(Pane::Threads, "focus-ring-threads"),
    );
    h_resizable("review-body")
        .with_state(&tab.panes.state)
        .child(
            resizable_panel().size_range(px(320.)..px(100_000.)).child(
                div()
                    .debug_selector(|| "viewport-pane".into())
                    .key_context("Viewport")
                    .track_focus(tab.viewport_focus())
                    // A click anywhere in the diff gives it the keyboard.
                    .capture_any_mouse_down(move |_, window, cx| window.focus(&focus, cx))
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .child(tab.viewport.clone())
                    .children(viewport_ring),
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
                            .relative()
                            .size_full()
                            .border_l_1()
                            .border_color(border)
                            .child(threads)
                            .children(threads_ring),
                    )
                }),
        )
        .into_any_element()
}

/// The ring around the pane that has the keyboard: drawn over the pane's
/// edge, and it takes no clicks.
fn focus_ring(selector: &'static str, cx: &Context<ReviewTab>) -> AnyElement {
    div()
        .debug_selector(move || selector.into())
        .absolute()
        .inset_0()
        .border_2()
        .border_color(cx.theme().ring.opacity(0.7))
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
