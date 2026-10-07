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
//! Spacing (ADR-0031 M1–M3): the main column starts after the sidebar's
//! 1 pt divider; both top rows hold their items on `edge::CANVAS` from their
//! trailing edge, the toolbar from its leading edge too (the cards' edge),
//! or after the traffic lights (`layout::TOOLBAR_INSET_HIDDEN`) while the
//! sidebar is hidden; items are `gap::CONTROLS` apart. The custom controls
//! here show press ink ([`ink_layer`]).
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
use gpui_kit::component::{
    ActiveTheme as _, Icon, ResizableState, Sizable as _, h_flex, h_resizable, resizable_panel,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, FontFeatures, Global,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Role, Stateful,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, point, px, size,
};

use crate::motion::ink::PressInk as _;
use crate::segmented::{SegmentSpec, segmented};
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
    /// The sidebar | main column split: its first panel's size is the
    /// sidebar's stored width.
    shell: Entity<ResizableState>,
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
        let shell = cx.new(|_| ResizableState::default());
        let chrome = cx.new(|_| Chrome {
            segment: Segment::Files,
            sidebar_visible: true,
            shell,
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

    pub fn set_sidebar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.sidebar_visible != visible {
            self.sidebar_visible = visible;
            cx.notify();
        }
    }

    /// Whether a review page draws its tree (or the find pane in its
    /// place): the sidebar shows, on Files.
    pub fn files_shown(&self) -> bool {
        self.sidebar_visible && self.segment == Segment::Files
    }

    /// The sidebar | main column split's state (tests resize it as a drag
    /// of the sidebar's edge does).
    pub fn shell(&self) -> &Entity<ResizableState> {
        &self.shell
    }
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

/// The main column's width in `window` as [`shell`] lays it out: the
/// window's, less the sidebar's and its divider's while it shows (its
/// stored width, the default before the first layout, within
/// `layout::SIDEBAR_RANGE` and [`sidebar_max`]).
pub fn main_column_width(threads_shown: bool, window: &Window, cx: &App) -> f32 {
    let window_width = window.viewport_size().width.as_f32();
    let chrome = chrome(cx).read(cx);
    if !chrome.sidebar_visible {
        return window_width;
    }
    // Until the split is first laid out its sizes are placeholders, below
    // the sidebar's minimum.
    let stored = chrome
        .shell
        .read(cx)
        .sizes()
        .first()
        .map(|w| w.as_f32())
        .filter(|w| *w >= layout::SIDEBAR_RANGE.0)
        .unwrap_or(layout::SIDEBAR_WIDTH);
    window_width
        - stored.clamp(
            layout::SIDEBAR_RANGE.0,
            sidebar_max(window_width, threads_shown),
        )
        - stroke::BORDER
}

/// A page: `sidebar` (its top row included) and its 1 pt divider beside
/// `main`, or `main` alone while the sidebar is hidden. `threads_shown`:
/// the page's threads panel shows, so the main column needs
/// `layout::THREADS_MAIN_MIN_WIDTH`.
pub fn shell(
    sidebar: AnyElement,
    main: AnyElement,
    threads_shown: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (visible, state) = {
        let c = chrome(cx).read(cx);
        (c.sidebar_visible, c.shell.clone())
    };
    let main = div()
        .debug_selector(|| "main-column".into())
        .h_full()
        .min_w_0()
        .child(main);
    if !visible {
        return main.w_full().into_any_element();
    }
    keep_main_column_flexible(&state, cx);
    let window_width = window.viewport_size().width.as_f32();
    let max = sidebar_max(window_width, threads_shown);
    let theme = cx.theme();
    // The sidebar's divider (ADR-0031 M1): gpui-kit's split draws its
    // handle's line over the main panel's first point, so the main column
    // starts after it. This element paints the same line there and carries
    // the `sidebar-divider` selector, which the kit's handle cannot.
    let divider = div()
        .debug_selector(|| "sidebar-divider".into())
        .flex_none()
        .w(px(stroke::BORDER))
        .h_full()
        .bg(theme.border);
    h_resizable("shell")
        .with_state(&state)
        .child(
            resizable_panel()
                .size(px(layout::SIDEBAR_WIDTH))
                .size_range(px(layout::SIDEBAR_RANGE.0)..px(max))
                .flex_none()
                .child(
                    div()
                        .debug_selector(|| "sidebar".into())
                        .size_full()
                        .bg(theme.sidebar)
                        .child(sidebar),
                ),
        )
        .child(
            resizable_panel()
                .size_range(px(main_min_width(threads_shown) + stroke::BORDER)..Pixels::MAX)
                .child(div().flex().size_full().child(divider).child(main.flex_1())),
        )
        .into_any_element()
}

/// Keeps the main column without a size of its own, so the sidebar keeps its
/// stored width when the window resizes: gpui-kit's split rescales every
/// panel by the same ratio once all of them have a size, which the first
/// measurement and every drag give them.
fn keep_main_column_flexible(state: &Entity<ResizableState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if s.sizes().len() == 2 {
            s.reset_panel(1, cx);
        }
    });
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
    let segments = segmented(
        "sidebar-segments",
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
    );
    let toggle = icon_button(
        "toggle-sidebar",
        IconName::PanelLeft,
        ("Hide sidebar", "Hide sidebar (⌃⌘S)"),
        move |_, cx| chrome.update(cx, |c, cx| c.set_sidebar_visible(false, cx)),
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
    div()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(height::SM))
        .rounded(px(radius))
        .text_color(cx.theme().secondary_foreground)
        .child(ink_layer(radius, cx))
        .child(
            div()
                .debug_selector(move || format!("{id}-icon"))
                .flex()
                .child(Icon::new(icon).with_size(px(icon_size::ICON))),
        )
        .role(Role::Button)
        .aria_label(label)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .on_click(move |_, window, cx| on_click(window, cx))
        .tooltip(crate::review_tab::toolbar::tooltip(tooltip))
}

/// The main column's top row (design §11.4): `left` and `right` clusters
/// around room to drag. While the sidebar is hidden it keeps the traffic
/// lights clear (not in fullscreen) and starts with a "show sidebar" button.
pub fn toolbar_row(
    id: &'static str,
    left: Vec<AnyElement>,
    right: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let chrome = chrome(cx);
    let hidden = !chrome.read(cx).sidebar_visible;
    let inset = hidden && !window.is_fullscreen();
    let show = hidden.then(|| {
        icon_button(
            "show-sidebar",
            IconName::PanelLeftOpen,
            ("Show sidebar", "Show sidebar (⌃⌘S)"),
            move |_, cx| chrome.update(cx, |c, cx| c.set_sidebar_visible(true, cx)),
            cx,
        )
        .into_any_element()
    });
    let theme = cx.theme();
    let row = h_flex()
        .id(id)
        .debug_selector(move || id.into())
        .flex_none()
        .h(px(height::TOP))
        .w_full()
        .px(px(edge::CANVAS))
        .gap(px(gap::CONTROLS))
        .when(inset, |row| row.pl(px(layout::TOOLBAR_INSET_HIDDEN)))
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.title_bar_border);
    drag_region(row, id, window, cx)
        .child(cluster(show.into_iter().chain(left).collect()).min_w_0())
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
