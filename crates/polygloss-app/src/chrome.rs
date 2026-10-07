//! Window chrome (design §11.1, ADR-0026): the window's options (a
//! transparent titlebar with the traffic lights inset into the sidebar's top
//! row, no native tabs, opaque), the sidebar's state ([`Chrome`]: segment,
//! visibility, width) and the shell every page renders:
//!
//! ```text
//! ┌ sidebar (280 pt, 220–480, hideable) ┬ main column (min 320 pt) ──────────┐
//! │ ● ● ●       [Files|Reviews] [◧]    │ toolbar row: left …        … right │ 52 pt; both drag
//! │ the segment's content              │ the page                           │
//! └─────────────────────────────────────┴────────────────────────────────────┘
//! ```
//!
//! The sidebar is an app-owned split ([`crate::panel_split`]): it slides in
//! and out from the window's leading edge (ADR-0030 M1) when the pointer,
//! ⌃⌘S or the palette shows or hides it (OQ-58), and snaps for everything
//! else (⌘F, `/`, ⌘P and `⇥` into the tree, ShowFiles/ShowReviews, view
//! restore). Its track is sampled once per frame, first thing in the main
//! window's render ([`sample_sidebar`]), so the split, the toolbar and the
//! tree read one `f`.
//!
//! Spacing (ADR-0031 M1–M3): the main column starts after the sidebar's
//! 1 pt divider; both top rows hold their items on `edge::CANVAS` from their
//! trailing edge, the toolbar from its leading edge too (the cards' edge),
//! or after the traffic lights (`layout::TOOLBAR_INSET_HIDDEN`) while the
//! sidebar is hidden; items are `gap::CONTROLS` apart. While the sidebar
//! moves the toolbar's first item follows its edge and the show-sidebar
//! button stays at the inset, covered or uncovered by the sliding frame
//! ([`toolbar_row`]). The custom controls here show press ink
//! ([`ink_layer`]).
//!
//! Both top rows move the window when dragged and zoom (the system's
//! double-click setting) on a double click, from anywhere but a control: a
//! control claims its press (`Window::prevent_default` on mouse-down, as
//! gpui-component's `Button` does), so a press on a button never moves the
//! window. Like a titlebar, a press on a row leaves the keyboard where it
//! is. The sidebar gives way first: it never takes more than the window
//! minus the main column's minimum ([`sidebar_max`]), and its stored width
//! comes back when the window widens.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, FontFeatures, Global, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Role, Stateful,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, point, px, size,
};

use crate::motion::Initiator;
use crate::motion::ink::PressInk as _;
use crate::panel_split::{self, PanelEdge, PanelFrame, PanelOwner, PanelState, panel_split};
use crate::segmented::{SegmentSpec, SegmentedState, segmented};
use crate::space::{edge, gap, height, layout, radius, size as icon_size, stroke};

gpui_kit::actions!(
    window,
    [
        /// ⌃⌘S: hide or show the sidebar.
        ToggleSidebar,
        /// Show the sidebar on its Files segment.
        ShowFiles,
        /// Show the sidebar on its Reviews segment.
        ShowReviews,
    ]
);

/// What the sidebar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// The active review's files (design §11.5).
    Files,
    /// Home and the open reviews (design §11.2).
    Reviews,
}

/// The main window's sidebar: one segment, visibility and width for every
/// page, for the session (OQ-37): a window reopened from the Dock gets them
/// back.
pub struct Chrome {
    segment: Segment,
    sidebar_visible: bool,
    /// The sidebar's stored width and its motion (M1): the shell's
    /// [`panel_split`].
    sidebar: PanelState,
    /// The `[Files | Reviews]` control's selected fill (M11), shared by
    /// every page's sidebar.
    segments: Entity<SegmentedState>,
}

/// The [`Chrome`] of the main window (a GPUI global).
struct ChromeGlobal(Entity<Chrome>);

impl Global for ChromeGlobal {}

impl Chrome {
    /// Creates the main window's chrome, once per session, and makes it the
    /// one [`chrome`] returns (`MainWindow::new`).
    pub(crate) fn install(cx: &mut App) -> Entity<Chrome> {
        if let Some(ChromeGlobal(chrome)) = cx.try_global::<ChromeGlobal>() {
            return chrome.clone();
        }
        let segments = cx.new(|_| SegmentedState::new(0));
        let chrome = cx.new(|_| Chrome {
            segment: Segment::Files,
            sidebar_visible: true,
            sidebar: PanelState::new(layout::SIDEBAR_WIDTH, true),
            segments,
        });
        cx.set_global(ChromeGlobal(chrome.clone()));
        chrome
    }

    pub fn segment(&self) -> Segment {
        self.segment
    }

    pub fn set_segment(&mut self, segment: Segment, cx: &mut Context<Self>) {
        if self.segment != segment {
            self.segment = segment;
            cx.notify();
        }
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    /// Shows or hides the sidebar at once (Programmatic: ShowFiles, ⌘F,
    /// `/`, ⌘P, `⇥` into the tree, restore).
    pub fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.set_visible(visible, Initiator::Programmatic, cx);
    }

    /// Shows or hides the sidebar as `initiator` plays it (ADR-0030 M1): the
    /// pointer (its toggle, the show-sidebar button, the View menu) and the
    /// keyboard (⌃⌘S, the palette; OQ-58) slide it from where it is painted.
    /// The model changes now: the tree's keyboard and its `⇥` stop are gone
    /// from a closing sidebar on the commit frame.
    pub fn set_sidebar_visible_by(
        &mut self,
        visible: bool,
        initiator: Initiator,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_visible(visible, initiator, cx);
    }

    fn set_visible(&mut self, visible: bool, initiator: Initiator, cx: &mut Context<Self>) {
        if self.sidebar_visible != visible {
            self.sidebar_visible = visible;
            self.sidebar.retarget(visible, initiator, cx);
            cx.notify();
        }
    }

    /// Whether a review page draws its tree (or the find pane in its
    /// place): the sidebar shows, on Files.
    pub fn files_shown(&self) -> bool {
        self.sidebar_visible && self.segment == Segment::Files
    }

    /// The sidebar's frame this frame, in a window whose review shows its
    /// threads panel or not (its range follows the main column's minimum).
    pub fn sidebar_frame(&self, threads_shown: bool, window: &Window) -> PanelFrame {
        self.sidebar.frame(
            sidebar_range(window.viewport_size().width.as_f32(), threads_shown),
            window.scale_factor(),
        )
    }

    /// While the sidebar moves, its frame's widths at the motion's two
    /// ends (the toolbar keeps the items that fit at both).
    pub fn sidebar_ends(&self, threads_shown: bool, window: &Window) -> Option<(f32, f32)> {
        self.sidebar.ends(
            self.sidebar_visible,
            sidebar_range(window.viewport_size().width.as_f32(), threads_shown),
            window.scale_factor(),
        )
    }

    /// The sidebar's frame at rest in its model state (the motion's
    /// target).
    pub fn sidebar_rest_frame(&self, threads_shown: bool, window: &Window) -> f32 {
        self.sidebar.rest_frame(
            self.sidebar_visible,
            sidebar_range(window.viewport_size().width.as_f32(), threads_shown),
            window.scale_factor(),
        )
    }
}

impl PanelOwner for Chrome {
    fn panel(&self) -> &PanelState {
        &self.sidebar
    }

    fn panel_mut(&mut self) -> &mut PanelState {
        &mut self.sidebar
    }

    fn shown(&self) -> bool {
        self.sidebar_visible
    }

    fn background(&self, cx: &App) -> Hsla {
        cx.theme().sidebar
    }
}

/// Samples the sidebar's motion for this frame: the main window's render
/// calls it first, before any page reads the sidebar's frame.
pub(crate) fn sample_sidebar(window: &mut Window, cx: &mut App) {
    let chrome = chrome(cx);
    panel_split::sample(&chrome, || Box::new(ToggleSidebar), window, cx);
}

/// The main window's chrome. Panics before the main window first opened.
pub fn chrome(cx: &App) -> Entity<Chrome> {
    cx.global::<ChromeGlobal>().0.clone()
}

/// Shows the sidebar on its Files segment (⌘F, the filter, ⌘P and moving the
/// keyboard into the tree do this first).
pub fn show_files(_window: &mut Window, cx: &mut App) {
    chrome(cx).update(cx, |c, cx| {
        c.set_sidebar_visible(true, cx);
        c.set_segment(Segment::Files, cx);
    });
}

/// The main window's options: `bounds`, a transparent titlebar with the
/// traffic lights where AppKit puts them in a unified toolbar (their row
/// centred in the top rows), our own titlebar drag, an opaque background
/// (OQ-50), no native window tabs.
pub fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    let (x, y) = (
        layout::TRAFFIC_LIGHT_X,
        (height::TOP - layout::TRAFFIC_LIGHT) / 2.0,
    );
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(x), px(y))),
        }),
        app_owns_titlebar_drag: true,
        window_background: WindowBackgroundAppearance::Opaque,
        tabbing_identifier: None,
        window_min_size: Some(size(px(layout::WINDOW_MIN.0), px(layout::WINDOW_MIN.1))),
        focus: true,
        show: true,
        ..WindowOptions::default()
    }
}

/// The main column's minimum width.
fn main_min_width(threads_shown: bool) -> f32 {
    if threads_shown {
        layout::THREADS_MAIN_MIN_WIDTH
    } else {
        layout::MAIN_MIN_WIDTH
    }
}

/// The widest the sidebar may be in a `window_width` window: what the main
/// column's minimum and the sidebar's divider leave, within
/// `layout::SIDEBAR_RANGE`.
pub fn sidebar_max(window_width: f32, threads_shown: bool) -> f32 {
    (window_width - main_min_width(threads_shown) - stroke::BORDER)
        .clamp(layout::SIDEBAR_RANGE.0, layout::SIDEBAR_RANGE.1)
}

/// The sidebar's width range in a `window_width` window: from
/// `layout::SIDEBAR_RANGE`'s start to [`sidebar_max`].
pub fn sidebar_range(window_width: f32, threads_shown: bool) -> (f32, f32) {
    (
        layout::SIDEBAR_RANGE.0,
        sidebar_max(window_width, threads_shown),
    )
}

/// The main column's width in `window` once the sidebar is at rest in its
/// model state: the window's, less the sidebar's frame (its stored width
/// within [`sidebar_range`], and its divider) while it shows.
pub fn main_column_width(threads_shown: bool, window: &Window, cx: &App) -> f32 {
    window.viewport_size().width.as_f32()
        - chrome(cx)
            .read(cx)
            .sidebar_rest_frame(threads_shown, window)
}

/// A page: the sidebar (`sidebar` renders its content, top row included,
/// while its frame is open) and its 1 pt divider beside `main` (debug
/// selector `main-column`), one [`panel_split`] in every state.
/// `threads_shown`: the page's threads panel shows, so the main column
/// needs `layout::THREADS_MAIN_MIN_WIDTH`.
pub fn shell(
    sidebar: impl FnOnce(&mut Window, &mut App) -> AnyElement,
    main: AnyElement,
    threads_shown: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let main = div()
        .debug_selector(|| "main-column".into())
        .size_full()
        .child(main)
        .into_any_element();
    let range = sidebar_range(window.viewport_size().width.as_f32(), threads_shown);
    let content = move |window: &mut Window, cx: &mut App| {
        let fill = cx.theme().sidebar;
        div()
            .debug_selector(|| "sidebar".into())
            .size_full()
            .bg(fill)
            .child(sidebar(window, cx))
            .into_any_element()
    };
    panel_split(
        "sidebar",
        PanelEdge::Leading,
        &chrome(cx),
        range,
        content,
        main,
        window,
        cx,
    )
}

/// The sidebar's top row: the traffic lights' room, then (right-aligned) the
/// `[Files | Reviews]` segmented control and the sidebar toggle. Files is
/// disabled (and Reviews shows) while `files_enabled` is false (Home).
pub fn sidebar_top_row(files_enabled: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    let chrome = chrome(cx);
    let shown = match chrome.read(cx).segment {
        Segment::Files if files_enabled => Segment::Files,
        _ => Segment::Reviews,
    };
    let state = chrome.read(cx).segments.clone();
    let fullscreen = window.is_fullscreen();
    let spec = |id, icon, label: &'static str, enabled| SegmentSpec {
        id,
        icon,
        label: label.into(),
        tooltip: if enabled {
            label
        } else {
            "Open a review to see its files"
        }
        .into(),
        enabled,
    };
    let select = chrome.clone();
    // A Reduced fade of the sidebar's content: the selected fill's shadow
    // ramps by α³ (ADR-0030 Panels, rule 8).
    let fade = chrome.read(cx).sidebar.sampled().opacity;
    let segments = segmented(
        "sidebar-segments",
        &state,
        vec![
            spec("segment-files", IconName::ListTree, "Files", files_enabled),
            spec("segment-reviews", IconName::RotateCcwClock, "Reviews", true),
        ],
        match shown {
            Segment::Files => 0,
            Segment::Reviews => 1,
        },
        move |ix, _, cx| {
            let which = if ix == 0 {
                Segment::Files
            } else {
                Segment::Reviews
            };
            // The segment shown keeps the one stored (Reviews on Home).
            if which != shown {
                select.update(cx, |c, cx| c.set_segment(which, cx));
            }
        },
    )
    .faded(fade);
    let toggle = icon_button(
        "toggle-sidebar",
        IconName::PanelLeft,
        ("Hide sidebar", "Hide sidebar (⌃⌘S)"),
        move |window, cx| {
            let initiator = crate::motion::initiator(window);
            chrome.update(cx, |c, cx| {
                c.set_sidebar_visible_by(false, initiator, window, cx)
            })
        },
        cx,
    );
    let row = h_flex()
        .id("sidebar-top-row")
        .debug_selector(|| "sidebar-top-row".into())
        .flex_none()
        .h(px(height::TOP))
        .w_full()
        .pl(px(if fullscreen {
            edge::CANVAS
        } else {
            layout::TOOLBAR_INSET_HIDDEN
        }))
        .pr(px(edge::CANVAS));
    drag_region(row, "sidebar-top-row", window, cx)
        .child(div().flex_1())
        .child(cluster(vec![
            segments.into_any_element(),
            toggle.into_any_element(),
        ]))
        .into_any_element()
}

/// Press ink (ADR-0030) over whatever a custom control paints: a layer
/// filling the control (which is `relative()`), rounded at `radius`, put
/// first among its children so its content paints over it. On the control
/// itself the ink would replace a selected control's fill.
pub fn ink_layer(radius: f32, cx: &App) -> Stateful<Div> {
    div()
        .id("ink")
        .absolute()
        .inset_0()
        .rounded(px(radius))
        .press_ink(cx)
}

/// Tabular figures (OpenType `tnum`): a count in the UI font keeps its
/// width as its digits change (ADR-0031, no layout shift).
pub fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// A top row's icon button: `height::SM` square at its height's radius, a
/// `size::ICON` icon (debug selector `<id>-icon`), press ink, its
/// accessibility label and tooltip; a press on it never moves the window.
fn icon_button(
    id: &'static str,
    icon: IconName,
    (label, tooltip): (&'static str, &'static str),
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let radius = radius::for_height(height::SM);
    button_frame(id, cx)
        // First, so the icon paints over the ink.
        .child(ink_layer(radius, cx))
        .child(button_icon(id, icon))
        .role(Role::Button)
        .aria_label(label)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| on_click(window, cx))
        .tooltip(crate::review_tab::toolbar::tooltip(tooltip))
}

/// What [`icon_button`] paints, with no listeners: the button's picture (an
/// opening sidebar covers the show-sidebar button, inert, ADR-0030 M1).
fn icon_box(id: &'static str, icon: IconName, cx: &App) -> Stateful<Div> {
    button_frame(id, cx).child(button_icon(id, icon))
}

/// A top row's icon button's box: `height::SM` square at its height's
/// radius.
fn button_frame(id: &'static str, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(height::SM))
        .rounded(px(radius::for_height(height::SM)))
        .text_color(cx.theme().secondary_foreground)
}

/// A top row's button icon, `size::ICON` (debug selector `<id>-icon`).
fn button_icon(id: &'static str, icon: IconName) -> Div {
    div()
        .debug_selector(move || format!("{id}-icon"))
        .flex()
        .child(Icon::new(icon).with_size(px(icon_size::ICON)))
}

/// The toolbar's leading inset while the sidebar is hidden: after the
/// traffic lights, or `CANVAS` in fullscreen (ADR-0031 M2).
fn toolbar_inset(fullscreen: bool) -> f32 {
    if fullscreen {
        edge::CANVAS
    } else {
        layout::TOOLBAR_INSET_HIDDEN
    }
}

/// Where the toolbar's first item starts in its row while the sidebar's
/// frame is `f` wide: at window x `max(f + CANVAS, inset + SM + CONTROLS)`
/// (ADR-0030 Panels), so it follows the sidebar's edge, rests at the
/// cards' edge or after the show-sidebar button, and never passes under
/// the traffic lights.
pub fn toolbar_lead(f: f32, fullscreen: bool) -> f32 {
    (toolbar_inset(fullscreen) + height::SM + gap::CONTROLS - f).max(edge::CANVAS)
}

/// The main column's top row (design §11.4): `left` and `right` clusters
/// around room to drag, its first item [`toolbar_lead`] from its start.
/// The show-sidebar button sits at window x `inset` (after the traffic
/// lights; `CANVAS` in fullscreen) whenever the sidebar's frame leaves any
/// of it uncovered, clipped at the main column's edge: live while the
/// sidebar is hidden (closing included), inert while it opens.
/// `threads_shown`: the page's threads panel shows (the sidebar's range
/// follows it).
pub fn toolbar_row(
    id: &'static str,
    left: Vec<AnyElement>,
    right: Vec<AnyElement>,
    threads_shown: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let chrome = chrome(cx);
    let (f, hidden) = {
        let c = chrome.read(cx);
        (c.sidebar_frame(threads_shown, window).f, !c.sidebar_visible)
    };
    let fullscreen = window.is_fullscreen();
    let inset = toolbar_inset(fullscreen);
    let show = (f < inset + height::SM).then(|| {
        let (icon, label) = (
            IconName::PanelLeftOpen,
            ("Show sidebar", "Show sidebar (⌃⌘S)"),
        );
        let button = if hidden {
            icon_button(
                "show-sidebar",
                icon,
                label,
                move |window, cx| {
                    let initiator = crate::motion::initiator(window);
                    chrome.update(cx, |c, cx| {
                        c.set_sidebar_visible_by(true, initiator, window, cx)
                    })
                },
                cx,
            )
        } else {
            icon_box("show-sidebar", icon, cx)
        };
        button
            .absolute()
            .left(px(inset - f))
            .top(px((height::TOP - height::SM) / 2.0))
    });
    let theme = cx.theme();
    let row = h_flex()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .flex_none()
        .h(px(height::TOP))
        .w_full()
        // Clips the show-sidebar button at the main column's edge.
        .overflow_hidden()
        .pl(px(toolbar_lead(f, fullscreen)))
        .pr(px(edge::CANVAS))
        .gap(px(gap::CONTROLS))
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.title_bar_border);
    drag_region(row, id, window, cx)
        .children(show)
        .child(cluster(left).min_w_0())
        .child(div().flex_1())
        .child(cluster(right).flex_none())
        .into_any_element()
}

/// A top row's group of items, `gap::CONTROLS` apart. Its labels and gaps
/// move the window like the rest of the row; its controls claim their own
/// presses.
fn cluster(children: Vec<AnyElement>) -> Div {
    h_flex()
        .h_full()
        .gap(px(gap::CONTROLS))
        .items_center()
        .children(children)
}

/// What a press on a top row asks of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarGesture {
    /// A drag: the window follows the pointer (`Window::start_window_move`).
    Move,
    /// A double click: zoom, or minimize, as the system is set
    /// (`Window::titlebar_double_click`).
    DoubleClick,
}

/// Where the top rows send their gestures instead of the window (a GPUI
/// global).
struct GestureSink(Rc<dyn Fn(TitlebarGesture)>);

impl Global for GestureSink {}

/// Sends the top rows' gestures to `sink` instead of the window (tests: the
/// test platform can neither move nor zoom a window).
pub fn redirect_titlebar_gestures(sink: impl Fn(TitlebarGesture) + 'static, cx: &mut App) {
    cx.set_global(GestureSink(Rc::new(sink)));
}

fn perform(gesture: TitlebarGesture, window: &Window, cx: &App) {
    match cx.try_global::<GestureSink>() {
        Some(GestureSink(sink)) => sink(gesture),
        None => match gesture {
            TitlebarGesture::Move => window.start_window_move(),
            TitlebarGesture::DoubleClick => window.titlebar_double_click(),
        },
    }
}

/// Whether the left button went down on a top row (not on a control).
#[derive(Default)]
struct Drag {
    pressed: bool,
}

/// Makes `row` (element id `key`) move the window when dragged and zoom (or
/// minimize: the system's double-click setting) on a double click.
fn drag_region(
    row: Stateful<Div>,
    key: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let state = window.use_keyed_state(key, cx, |_, _| Drag::default());
    let (down, up, up_out, moved) = (state.clone(), state.clone(), state.clone(), state);
    let release = |state: &Entity<Drag>, cx: &mut App| state.update(cx, |d, _| d.pressed = false);
    row.on_mouse_down(MouseButton::Left, move |event, window, cx| {
        if window.default_prevented() {
            return; // A control's press.
        }
        // No focus-on-click above the row: the keyboard stays put.
        window.prevent_default();
        if event.click_count == 2 {
            perform(TitlebarGesture::DoubleClick, window, cx);
        } else {
            down.update(cx, |d, _| d.pressed = true);
        }
    })
    .on_mouse_up(MouseButton::Left, move |_, _, cx| release(&up, cx))
    .on_mouse_up_out(MouseButton::Left, move |_, _, cx| release(&up_out, cx))
    .on_mouse_move(move |event, window, cx| {
        if event.pressed_button == Some(MouseButton::Left)
            && moved.update(cx, |d, _| std::mem::take(&mut d.pressed))
        {
            perform(TitlebarGesture::Move, window, cx);
        }
    })
}
