//! The review tab's panes (design §11.1): the sidebar's content (T3.6's
//! `tree::render_pane`, find in its place, or the Reviews list) and the main
//! column's diff viewport | threads panel (T3.9's; hidden until shown), an
//! app-owned split ([`crate::panel_split`]): the panel slides in from the
//! window's trailing edge and back when the pointer (the toolbar's threads
//! button, the View menu) or the palette shows or hides it (ADR-0030 M2,
//! OQ-58), and snaps when it opens by itself (the composer, a panel-only
//! thread, agent replies, a URL or MCP `focus`, restore). While either
//! panel slides, the viewport's layout is held at its width once both
//! settle ([`hold_viewport`]).

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Styled as _, Window, div, px,
};

use crate::chrome::{self, Segment};
use crate::keyboard::Pane;
use crate::motion::Initiator;
use crate::panel_split::{self, PanelEdge, PanelOwner, PanelState, panel_split};
use crate::review_tab::ReviewTab;
use crate::space::{layout, stroke};

gpui_kit::actions!(
    tab,
    [
        /// Show or hide the threads panel.
        ToggleThreadsPanel,
    ]
);

/// The threads panel's stored width and motion (a [`PanelOwner`], one per
/// tab). Its frame is today's pane, the 1 pt divider its first point, so W
/// is the pane's width less that divider.
pub struct ThreadsPanel {
    shown: bool,
    panel: PanelState,
}

impl PanelOwner for ThreadsPanel {
    fn panel(&self) -> &PanelState {
        &self.panel
    }

    fn panel_mut(&mut self) -> &mut PanelState {
        &mut self.panel
    }

    fn shown(&self) -> bool {
        self.shown
    }

    fn background(&self, cx: &App) -> Hsla {
        crate::theme::viewport_theme(cx).canvas
    }
}

/// Pane state of one review tab.
pub(crate) struct Panes {
    /// Whether the threads panel shows: `None` until it is shown or hidden
    /// (then it is hidden), as view state keeps it (design §11.12).
    pub threads_panel: Option<bool>,
    /// The threads panel's width and motion (kept while the tab lives).
    pub threads: Entity<ThreadsPanel>,
    /// The viewport's outer width its layout is held at while a panel
    /// slides ([`hold_viewport`]).
    held: Option<f32>,
}

impl Panes {
    pub fn new(cx: &mut Context<ReviewTab>) -> Panes {
        let threads = cx.new(|_| ThreadsPanel {
            shown: false,
            panel: PanelState::new(layout::THREADS_WIDTH - stroke::BORDER, false),
        });
        Panes {
            threads_panel: None,
            threads,
            held: None,
        }
    }

    pub fn threads_shown(&self) -> bool {
        self.threads_panel == Some(true)
    }

    /// Shows or hides the threads panel as `initiator` plays it. Whether it
    /// changed.
    pub fn set_threads_shown(&mut self, shown: bool, initiator: Initiator, cx: &mut App) -> bool {
        if self.threads_shown() == shown {
            return false;
        }
        self.threads_panel = Some(shown);
        self.threads.update(cx, |t, cx| {
            t.shown = shown;
            t.panel.retarget(shown, initiator, cx);
            cx.notify();
        });
        true
    }
}

impl Panes {
    /// The threads panel as the view state saved it (`restore`): shown or
    /// hidden at once, the choice kept as made.
    pub fn restore_threads(&mut self, shown: bool, cx: &mut App) {
        self.set_threads_shown(shown, Initiator::Programmatic, cx);
        self.threads_panel = Some(shown);
    }
}

impl ReviewTab {
    /// The threads panel's [`PanelOwner`] (tests size it through its
    /// state, as a drag of its divider does).
    pub fn threads_panel(&self) -> &Entity<ThreadsPanel> {
        &self.panes.threads
    }
}

/// The threads panel's width range in `window`: today's pane range less its
/// divider, up to what leaves the viewport `VIEWPORT_MIN_WIDTH` of the main
/// column once the sidebar is at rest (design §11.1).
fn threads_range(window: &Window, cx: &App) -> (f32, f32) {
    let main = chrome::main_column_width(true, window, cx);
    let (min, max) = layout::THREADS_RANGE;
    let max = (main - layout::VIEWPORT_MIN_WIDTH).clamp(min, max);
    (min - stroke::BORDER, max - stroke::BORDER)
}

/// Samples the threads panel's motion for this frame (the tab's render,
/// before its panes).
pub(crate) fn sample_threads(tab: &ReviewTab, window: &mut Window, cx: &mut App) {
    panel_split::sample(
        &tab.panes.threads,
        || Box::new(ToggleThreadsPanel),
        window,
        cx,
    );
}

/// While the sidebar or the threads panel slides (Full), holds what the
/// viewport reshapes at the outer width it has once both are at rest, from
/// the commit frame until they settle (ADR-0030 Panels); Reduced holds
/// nothing, its width changing once.
pub(crate) fn hold_viewport(tab: &mut ReviewTab, window: &mut Window, cx: &mut App) {
    let shown = tab.panes.threads_shown();
    let chrome = chrome::chrome(cx);
    let sliding = panel_split::holds_layout(cx)
        && (chrome.read(cx).panel().sliding() || tab.panes.threads.read(cx).panel.sliding());
    if sliding {
        let threads = tab.panes.threads.read(cx).panel.rest_frame(
            shown,
            threads_range(window, cx),
            window.scale_factor(),
        );
        let target = chrome::main_column_width(shown, window, cx) - threads;
        if tab.panes.held != Some(target) {
            tab.panes.held = Some(target);
            tab.viewport
                .update(cx, |v, cx| v.hold_layout(px(target), cx));
        }
    } else if tab.panes.held.take().is_some() {
        tab.viewport.update(cx, |v, cx| v.release_layout(cx));
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

/// The main column's panes of `tab`: the diff viewport | the threads panel,
/// one [`panel_split`] (row `threads`; the frame's selector `threads-pane`).
/// The panel keeps its stored width (up to `THREADS_RANGE`'s end) while the
/// main column leaves the viewport `VIEWPORT_MIN_WIDTH`, and gives way down
/// to the range's start ([`layout`]); the stored width comes back when the
/// window widens.
pub(crate) fn render(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let range = threads_range(window, cx);
    let open = tab
        .panes
        .threads
        .read(cx)
        .panel
        .frame(range, window.scale_factor())
        .f
        > 0.0;
    let focused = ringed_pane(tab, window, cx);
    let ring = |pane: Pane, selector: &'static str| {
        (focused.as_ref() == Some(&pane)).then(|| focus_ring(selector, cx))
    };
    let (viewport_ring, threads_ring) = (
        ring(Pane::Viewport, "focus-ring-viewport"),
        ring(Pane::Threads, "focus-ring-threads"),
    );
    // The panel's content, while its frame is open.
    let canvas = crate::theme::viewport_theme(cx).canvas;
    let threads = open.then(|| {
        let panel = crate::threads::render_panel(tab, window, cx)
            .unwrap_or_else(|| div().size_full().into_any_element());
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(canvas)
            .child(panel)
            .children(threads_ring)
            .into_any_element()
    });
    let focus = tab.viewport_focus().clone();
    let viewport = div()
        .debug_selector(|| "viewport-pane".into())
        .key_context("Viewport")
        .track_focus(tab.viewport_focus())
        // A click anywhere in the diff gives it the keyboard.
        .capture_any_mouse_down(move |_, window, cx| window.focus(&focus, cx))
        .relative()
        .size_full()
        .overflow_hidden()
        .child(tab.viewport.clone())
        .children(viewport_ring)
        .into_any_element();
    panel_split(
        "threads",
        PanelEdge::Trailing,
        &tab.panes.threads,
        range,
        move |_, _| threads.unwrap_or_else(|| div().into_any_element()),
        viewport,
        window,
        cx,
    )
}

/// The ring around the pane that has the keyboard: drawn over the pane's
/// edge, and it takes no clicks.
fn focus_ring(selector: &'static str, cx: &Context<ReviewTab>) -> AnyElement {
    div()
        .debug_selector(move || selector.into())
        .absolute()
        .inset_0()
        .border(px(stroke::FOCUS_RING))
        .border_color(cx.theme().ring.opacity(0.7))
        .into_any_element()
}
