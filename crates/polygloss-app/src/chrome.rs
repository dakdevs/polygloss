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
//! Both top rows move the window when dragged and zoom (the system's
//! double-click setting) on a double click, from anywhere but a control: a
//! control claims its press (`Window::prevent_default` on mouse-down, as
//! gpui-component's `Button` does), so a press on a button never moves the
//! window. Like a titlebar, a press on a row leaves the keyboard where it
//! is. The sidebar gives way first: it never takes more than the window
//! minus the main column's minimum ([`sidebar_max`]), and its stored width
//! comes back when the window widens.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, ResizableState, Sizable as _, h_flex, h_resizable, resizable_panel,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, Global,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Stateful,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, point, px, size,
};

/// The height of both top rows: the sidebar's and the main column's toolbar.
pub const TOP_ROW_HEIGHT: f32 = 52.0;
/// Where AppKit puts the close button: a 14 pt button centred in the top row
/// (tuned by eye in T6.16).
pub const TRAFFIC_LIGHT_POSITION: (f32, f32) = (19.0, 19.0);
/// The width the traffic lights take at the left of a top row.
pub const TRAFFIC_LIGHT_INSET: f32 = 80.0;
/// The sidebar's width until the user drags its edge.
pub const SIDEBAR_WIDTH: f32 = 280.0;
/// The widths the sidebar may take.
pub const SIDEBAR_RANGE: (f32, f32) = (220.0, 480.0);
/// The main column's minimum width.
pub const MAIN_MIN_WIDTH: f32 = 320.0;
/// The main column's minimum while the threads panel shows: viewport 260 +
/// threads panel 220.
pub const THREADS_MAIN_MIN_WIDTH: f32 = 480.0;
/// The smallest window (720 pt holds a 220 pt sidebar beside 480 pt).
const WINDOW_MIN_SIZE: (f32, f32) = (720.0, 480.0);

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
/// traffic lights at [`TRAFFIC_LIGHT_POSITION`], our own titlebar drag, an
/// opaque background (OQ-50), no native window tabs.
pub fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    let (x, y) = TRAFFIC_LIGHT_POSITION;
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
        window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
        focus: true,
        show: true,
        ..WindowOptions::default()
    }
}

/// The main column's minimum width.
fn main_min_width(threads_shown: bool) -> f32 {
    if threads_shown {
        THREADS_MAIN_MIN_WIDTH
    } else {
        MAIN_MIN_WIDTH
    }
}

/// The widest the sidebar may be in a `window_width` window: what the main
/// column's minimum leaves, within [`SIDEBAR_RANGE`].
pub fn sidebar_max(window_width: f32, threads_shown: bool) -> f32 {
    (window_width - main_min_width(threads_shown)).clamp(SIDEBAR_RANGE.0, SIDEBAR_RANGE.1)
}

/// A page: `sidebar` (its top row included) beside `main`, or `main` alone
/// while the sidebar is hidden. `threads_shown`: the page's threads panel
/// shows, so the main column needs [`THREADS_MAIN_MIN_WIDTH`].
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
        .size_full()
        .min_w_0()
        .child(main);
    if !visible {
        return main.into_any_element();
    }
    keep_main_column_flexible(&state, cx);
    let window_width = window.viewport_size().width.as_f32();
    let max = sidebar_max(window_width, threads_shown);
    let theme = cx.theme();
    h_resizable("shell")
        .with_state(&state)
        .child(
            resizable_panel()
                .size(px(SIDEBAR_WIDTH))
                .size_range(px(SIDEBAR_RANGE.0)..px(max))
                .flex_none()
                .child(
                    div()
                        .debug_selector(|| "sidebar".into())
                        .size_full()
                        .bg(theme.sidebar)
                        .border_r_1()
                        .border_color(theme.sidebar_border)
                        .child(sidebar),
                ),
        )
        .child(
            resizable_panel()
                .size_range(px(main_min_width(threads_shown))..Pixels::MAX)
                .child(main),
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
    let theme = cx.theme();
    let segment = |id: &'static str, icon: IconName, label: &'static str, which: Segment| {
        let selected = shown == which;
        let enabled = which == Segment::Reviews || files_enabled;
        let tooltip: &'static str = if enabled {
            label
        } else {
            "Open a review to see its files"
        };
        let chrome = chrome.clone();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(24.))
            .rounded(px(5.))
            .text_color(if !enabled {
                theme.muted_foreground.opacity(0.4)
            } else if selected {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .when(selected, |el| el.bg(theme.tab_active).shadow_xs())
            .when(enabled && !selected, |el| {
                let hover = theme.foreground;
                el.hover(move |s| s.text_color(hover))
                    .on_click(move |_, _, cx| {
                        chrome.update(cx, |c, cx| c.set_segment(which, cx));
                    })
            })
            .child(Icon::new(icon).small())
            .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
    };
    let segments = h_flex()
        .debug_selector(|| "sidebar-segments".into())
        // A control: a press on it (a segment, a disabled one, the rim)
        // never moves the window.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(7.))
        .bg(theme.tab_bar_segmented)
        .child(segment(
            "segment-files",
            IconName::ListTree,
            "Files",
            Segment::Files,
        ))
        .child(segment(
            "segment-reviews",
            IconName::RotateCcwClock,
            "Reviews",
            Segment::Reviews,
        ));
    let toggle = Button::new("toggle-sidebar")
        .icon(IconName::PanelLeft)
        .ghost()
        .small()
        .tooltip("Hide sidebar")
        .debug_selector(|| "toggle-sidebar".into())
        .on_click(move |_, _, cx| {
            chrome.update(cx, |c, cx| c.set_sidebar_visible(false, cx));
        });
    let row = h_flex()
        .id("sidebar-top-row")
        .debug_selector(|| "sidebar-top-row".into())
        .flex_none()
        .h(px(TOP_ROW_HEIGHT))
        .w_full()
        .pl(px(if fullscreen { 12. } else { TRAFFIC_LIGHT_INSET }))
        .pr_2();
    drag_region(row, "sidebar-top-row", window, cx)
        .child(div().flex_1())
        .child(cluster(vec![
            segments.into_any_element(),
            toggle.into_any_element(),
        ]))
        .into_any_element()
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
        Button::new("show-sidebar")
            .icon(IconName::PanelLeftOpen)
            .ghost()
            .small()
            .tooltip("Show sidebar")
            .debug_selector(|| "show-sidebar".into())
            .on_click(move |_, _, cx| {
                chrome.update(cx, |c, cx| c.set_sidebar_visible(true, cx));
            })
            .into_any_element()
    });
    let theme = cx.theme();
    let row = h_flex()
        .id(id)
        .debug_selector(move || id.into())
        .flex_none()
        .h(px(TOP_ROW_HEIGHT))
        .w_full()
        .px_3()
        .gap_2()
        .when(inset, |row| row.pl(px(TRAFFIC_LIGHT_INSET)))
        .bg(theme.title_bar)
        .border_b_1()
        .border_color(theme.title_bar_border);
    drag_region(row, id, window, cx)
        .child(cluster(show.into_iter().chain(left).collect()).min_w_0())
        .child(div().flex_1())
        .child(cluster(right).flex_none())
        .into_any_element()
}

/// A top row's group of items, 8 pt apart. Its labels and gaps move the
/// window like the rest of the row; its controls claim their own presses.
fn cluster(children: Vec<AnyElement>) -> Div {
    h_flex().h_full().gap_2().items_center().children(children)
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
