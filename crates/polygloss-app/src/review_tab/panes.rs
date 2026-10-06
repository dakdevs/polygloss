//! The review tab's panes (design §11.1): the sidebar's content (T3.6's
//! `tree::render_pane`, find in its place, or the Reviews list) and the main
//! column's resizable diff viewport | threads panel (T3.9's; hidden until
//! shown, its content sliding in when the toolbar's button opens it, design
//! §11.16).

use std::time::Instant;

use gpui_kit::component::{ActiveTheme as _, ResizableState, h_resizable, resizable_panel, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Styled as _, Window, div, point, px,
};

use crate::chrome::{self, Segment};
use crate::keyboard::Pane;
use crate::motion;
use crate::review_tab::ReviewTab;
use crate::space::{layout, stroke};

gpui_kit::actions!(
    tab,
    [
        /// Show or hide the threads panel.
        ToggleThreadsPanel,
    ]
);

/// How far right of its place the panel's content starts entering.
const ENTER_FROM_RIGHT: f32 = 12.0;

/// Pane state of one review tab.
pub(crate) struct Panes {
    /// Whether the threads panel shows: `None` until it is shown or hidden
    /// (then it is hidden), as view state keeps it (design §11.12).
    pub threads_panel: Option<bool>,
    /// The panel content's entrance after a pointer open: its epoch, and
    /// when it is over on the executor clock.
    entrance: Option<(u64, Instant)>,
    /// Entrances started so far: each plays once.
    entrances: u64,
    /// The viewport | threads panel widths (kept while the tab lives).
    pub state: Entity<ResizableState>,
}

impl Panes {
    pub fn new(cx: &mut Context<ReviewTab>) -> Panes {
        // gpui-base stores a panel's first measured width while its size is
        // still the 100 pt placeholder. Both start with real sizes instead:
        // the panel keeps 340 pt even when first shown in a narrow window
        // (which clamps it), and the viewport never gets a width of its own
        // ([`render`] clears it every frame), so resizing the window never
        // rescales the panel's.
        let state = cx.new(|cx| {
            let mut state = ResizableState::default();
            state.insert_panel(Some(px(layout::VIEWPORT_MIN_WIDTH)), None, cx);
            state.insert_panel(Some(px(layout::THREADS_WIDTH)), None, cx);
            state
        });
        Panes {
            threads_panel: None,
            entrance: None,
            entrances: 0,
            state,
        }
    }

    pub fn threads_shown(&self) -> bool {
        self.threads_panel == Some(true)
    }

    /// Shows or hides the threads panel at once, dropping an entrance that
    /// was playing. Whether it changed.
    pub fn set_threads_shown(&mut self, shown: bool) -> bool {
        if self.threads_shown() == shown {
            return false;
        }
        self.threads_panel = Some(shown);
        self.entrance = None;
        true
    }

    /// Plays the panel content's entrance from `now` (design §11.16).
    pub fn enter_threads(&mut self, now: Instant) {
        self.entrances += 1;
        self.entrance = Some((self.entrances, now + motion::ENTER_PANEL));
    }

    /// The epoch of the entrance playing at `now`. The content is wrapped in
    /// it only while it plays: `enter_from` would replay it on the next
    /// frame that draws it after one that did not (another review shown).
    fn entering(&self, now: Instant) -> Option<u64> {
        self.entrance
            .filter(|(_, until)| now < *until)
            .map(|(epoch, _)| epoch)
    }
}

impl ReviewTab {
    /// The viewport | threads panel split's state (tests size the panel
    /// through it, as dragging its edge does).
    pub fn threads_split(&self) -> &Entity<ResizableState> {
        &self.panes.state
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

/// The main column's panes of `tab`: diff viewport | threads panel. The
/// panel keeps its stored width (up to `THREADS_RANGE`'s end) while the
/// main column leaves the viewport `VIEWPORT_MIN_WIDTH`, and gives way down
/// to the range's start (design §11.1, [`layout`]); the stored width comes
/// back when the window widens.
pub(crate) fn render(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let shown = tab.panes.threads_shown();
    let threads = shown.then(|| {
        let panel = crate::threads::render_panel(tab, window, cx)
            .unwrap_or_else(|| div().size_full().into_any_element());
        let content = div().size_full().child(panel);
        match tab.panes.entering(cx.background_executor().now()) {
            Some(epoch) => motion::enter_from(
                "threads-panel",
                epoch,
                point(px(ENTER_FROM_RIGHT), px(0.)),
                motion::ENTER_PANEL,
                motion::ease_out_quint,
                content,
                window,
                cx,
            ),
            None => content.into_any_element(),
        }
    });
    let main = chrome::main_column_width(shown, window, cx);
    let (threads_min, threads_max) = layout::THREADS_RANGE;
    let threads_max = (main - layout::VIEWPORT_MIN_WIDTH).clamp(threads_min, threads_max);
    // The viewport keeps no width of its own (a drag of the panel's edge
    // gives it one), so resizing the window never rescales the panel's.
    tab.panes.state.update(cx, |state, cx| {
        if state.sizes().len() == 2 {
            state.reset_panel(0, cx);
        }
    });
    let theme = cx.theme();
    let (border, canvas) = (theme.border, crate::theme::viewport_theme(cx).canvas);
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
            resizable_panel()
                .size_range(px(layout::VIEWPORT_MIN_WIDTH)..Pixels::MAX)
                .child(
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
                .size(px(layout::THREADS_WIDTH))
                .size_range(px(threads_min)..px(threads_max))
                .flex_none()
                .visible(threads.is_some())
                .when_some(threads, |panel, threads| {
                    panel.child(
                        div()
                            .debug_selector(|| "threads-pane".into())
                            .relative()
                            .size_full()
                            .overflow_hidden()
                            .bg(canvas)
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
        .border(px(stroke::FOCUS_RING))
        .border_color(cx.theme().ring.opacity(0.7))
        .into_any_element()
}
