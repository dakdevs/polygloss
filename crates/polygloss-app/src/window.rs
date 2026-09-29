//! The one main window (design §11.1, ADR-0023): a title bar holding the tab
//! bar, then the active tab. The app keeps running when it closes, and
//! clicking the Dock icon reopens it ([`reopen`], `on_reopen`). Also the
//! native menu bar skeleton (App, File, Edit, View, Review, Window, Help),
//! which feature modules extend with [`add_menu_items`] before
//! [`install_menus`] runs.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, TitleBar, WindowExt as _, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, KeyBinding, Menu, MenuItem, OsAction, ParentElement as _,
    Render, SharedString, Styled as _, SystemMenuType, WeakEntity, Window, WindowBounds,
    WindowOptions, div, px, size,
};

use crate::home::HomeView;
use crate::review_tab::{ReviewTab, panes::ToggleThreadsPanel};
use crate::settings::SettingsStore;
use crate::tabs::{CloseTab, NextTab, PrevTab, TabItem, Tabs};

gpui_kit::actions!(
    window,
    [
        /// ⌘Q.
        Quit,
        /// ⌘M.
        Minimize,
        /// Window › Zoom.
        Zoom,
    ]
);

/// The window's size the first time it opens, in points.
const WINDOW_SIZE: (f32, f32) = (1440.0, 900.0);

/// The main window and its root view (a GPUI global while it is open).
struct MainWindowHandle {
    window: AnyWindowHandle,
    view: WeakEntity<MainWindow>,
}

impl Global for MainWindowHandle {}

/// The main window's root view: the tabs.
pub struct MainWindow {
    focus: FocusHandle,
    tabs: Tabs,
    /// Every open error shown (newest last), for tests and diagnostics.
    open_errors: Vec<SharedString>,
    /// Every toast shown (newest last).
    toasts: Vec<SharedString>,
    /// The settings error generation last toasted.
    settings_errors_seen: u64,
}

/// Key bindings and handlers of the window and its tabs.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    add_menu_items(
        MenuKind::App,
        vec![
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Polygloss", Quit),
        ],
        cx,
    );
    add_menu_items(
        MenuKind::File,
        vec![MenuItem::action("Close Tab", CloseTab)],
        cx,
    );
    add_menu_items(
        MenuKind::Edit,
        vec![
            MenuItem::os_action("Undo", gpui_kit::component::input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", gpui_kit::component::input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", gpui_kit::component::input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", gpui_kit::component::input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", gpui_kit::component::input::Paste, OsAction::Paste),
            MenuItem::os_action(
                "Select All",
                gpui_kit::component::input::SelectAll,
                OsAction::SelectAll,
            ),
        ],
        cx,
    );
    add_menu_items(
        MenuKind::View,
        vec![MenuItem::action("Toggle Threads Panel", ToggleThreadsPanel)],
        cx,
    );
    add_menu_items(
        MenuKind::Window,
        vec![
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PrevTab),
        ],
        cx,
    );
}

/// The menus of the menu bar, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    App,
    File,
    Edit,
    View,
    Review,
    Window,
    Help,
}

impl MenuKind {
    const ALL: [MenuKind; 7] = [
        MenuKind::App,
        MenuKind::File,
        MenuKind::Edit,
        MenuKind::View,
        MenuKind::Review,
        MenuKind::Window,
        MenuKind::Help,
    ];

    fn title(self) -> &'static str {
        match self {
            MenuKind::App => "Polygloss",
            MenuKind::File => "File",
            MenuKind::Edit => "Edit",
            MenuKind::View => "View",
            MenuKind::Review => "Review",
            MenuKind::Window => "Window",
            MenuKind::Help => "Help",
        }
    }
}

/// Menu items contributed by the features, per menu.
#[derive(Default)]
struct MenuItems(Vec<(MenuKind, Vec<MenuItem>)>);

impl Global for MenuItems {}

/// Appends `items` to menu `kind` (call from a feature's `init`; the menu
/// bar is built once, by [`install_menus`]).
pub fn add_menu_items(kind: MenuKind, items: Vec<MenuItem>, cx: &mut App) {
    cx.default_global::<MenuItems>().0.push((kind, items));
}

/// The menu bar: every menu, with the items the features added in `init`
/// order (a menu nobody added to still shows, empty, as the skeleton). Takes
/// the contributions: the bar is built once.
pub fn menus(cx: &mut App) -> Vec<Menu> {
    let mut contributed = std::mem::take(&mut cx.default_global::<MenuItems>().0);
    MenuKind::ALL
        .iter()
        .map(|&kind| Menu {
            name: kind.title().into(),
            items: contributed
                .iter_mut()
                .filter(|(k, _)| *k == kind)
                .flat_map(|(_, items)| std::mem::take(items))
                .collect(),
            disabled: false,
        })
        .collect()
}

/// Sets the menu bar.
pub fn install_menus(cx: &mut App) {
    let menus = menus(cx);
    cx.set_menus(menus);
}

/// The main window and its view, while it is open.
pub fn main_window(cx: &App) -> Option<(AnyWindowHandle, Entity<MainWindow>)> {
    let handle = cx.try_global::<MainWindowHandle>()?;
    let view = handle.view.upgrade()?;
    cx.windows()
        .contains(&handle.window)
        .then_some((handle.window, view))
}

/// Opens the main window with its Home tab.
pub fn open_main_window(cx: &mut App) -> anyhow::Result<(AnyWindowHandle, Entity<MainWindow>)> {
    open_main_window_sized(size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx)
}

/// [`open_main_window`] at `window_size` (points; screenshots pin it).
pub fn open_main_window_sized(
    window_size: gpui_kit::Size<gpui_kit::Pixels>,
    cx: &mut App,
) -> anyhow::Result<(AnyWindowHandle, Entity<MainWindow>)> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(window_size, cx)),
        window_min_size: Some(size(px(720.), px(480.))),
        focus: true,
        show: true,
        ..TitleBar::window_options()
    };
    let (window, view) = gpui_kit::open_window(options, cx, |window, cx| {
        let home = HomeView::new(window, cx);
        cx.new(|cx| MainWindow::new(home, window, cx))
    })?;
    cx.set_global(MainWindowHandle {
        window,
        view: view.downgrade(),
    });
    Ok((window, view))
}

/// `on_reopen` (the Dock icon clicked, the app launched again): opens the
/// main window unless it is open.
pub fn reopen(cx: &mut App) {
    if main_window(cx).is_some() {
        return;
    }
    if let Err(e) = open_main_window(cx) {
        tracing::error!("reopening the main window: {e:#}");
    }
}

impl MainWindow {
    fn new(home: Entity<HomeView>, window: &mut Window, cx: &mut Context<Self>) -> MainWindow {
        let focus = cx.focus_handle();
        window.focus(&home.read(cx).focus_handle(cx), cx);
        cx.observe_global_in::<SettingsStore>(window, |this, window, cx| {
            this.toast_settings_error(window, cx)
        })
        .detach();
        let mut this = MainWindow {
            focus,
            tabs: Tabs::new(home),
            open_errors: Vec::new(),
            toasts: Vec::new(),
            settings_errors_seen: 0,
        };
        this.toast_settings_error(window, cx);
        this
    }

    pub fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    /// Messages of failed opens, oldest first.
    pub fn open_errors(&self) -> &[SharedString] {
        &self.open_errors
    }

    /// Toasts shown, oldest first.
    pub fn toasts(&self) -> &[SharedString] {
        &self.toasts
    }

    /// Shows `message` as an error toast.
    pub fn toast_error(
        &mut self,
        message: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toasts.push(message.clone());
        window.push_notification(Notification::error(message), cx);
        cx.notify();
    }

    /// Reports a review that could not be opened.
    pub fn show_error(&mut self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        let message: SharedString = message.into();
        self.open_errors.push(message.clone());
        self.toast_error(message, window, cx);
    }

    fn toast_settings_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = SettingsStore::global(cx);
        let generation = store.error_generation();
        if generation == self.settings_errors_seen {
            return;
        }
        self.settings_errors_seen = generation;
        if let Some(err) = store.last_error() {
            let message = format!("Invalid settings, keeping the previous ones: {err}");
            self.toast_error(message.into(), window, cx);
        }
    }

    /// Adds a review tab and activates it.
    pub(crate) fn push_review(
        &mut self,
        tab: Entity<ReviewTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let ix = self.tabs.push_review(tab);
        self.focus_active(window, cx);
        cx.notify();
        ix
    }

    /// Activates tab `ix`.
    pub fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate(ix);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Moves keyboard focus into the active tab.
    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = match self.tabs.active_item() {
            TabItem::Home(home) => home.read(cx).focus_handle(cx),
            TabItem::Review(tab) => tab.read(cx).focus_handle(cx),
        };
        window.focus(&handle, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.tabs.active();
        if self.tabs.closable(active) {
            self.tabs.close(active);
            self.focus_active(window, cx);
            cx.notify();
        } else {
            // Home: ⌘W closes the window, as in any Mac app; the app keeps
            // running.
            window.remove_window();
        }
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.close(ix).is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.next();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.prev();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.items().iter().enumerate().map(|(ix, item)| {
            let tab =
                Tab::new()
                    .label(item.title(cx))
                    .when(matches!(item, TabItem::Home(_)), |t| {
                        t.prefix(
                            div()
                                .pl_1()
                                .child(gpui_kit::component::Icon::new(IconName::Inbox).small()),
                        )
                    });
            if self.tabs.closable(ix) {
                tab.suffix(
                    Button::new(("close-tab", ix))
                        .icon(IconName::Close)
                        .xsmall()
                        .ghost()
                        .tooltip("Close Tab (⌘W)")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_tab_at(ix, window, cx)
                        })),
                )
            } else {
                tab
            }
        });
        TabBar::new("tabs")
            .selected_index(self.tabs.active())
            .max_width(px(260.))
            .children(tabs)
            .on_click(
                cx.listener(|this, ix: &usize, window, cx| this.activate_tab(*ix, window, cx)),
            )
    }
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active: gpui_kit::AnyElement = match self.tabs.active_item() {
            TabItem::Home(home) => home.clone().into_any_element(),
            TabItem::Review(tab) => tab.clone().into_any_element(),
        };
        let theme = cx.theme();
        v_flex()
            .key_context("Window")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                TitleBar::new().child(
                    div()
                        .debug_selector(|| "tab-bar".into())
                        .flex_1()
                        .h_full()
                        .min_w_0()
                        .child(self.render_tab_bar(cx)),
                ),
            )
            .child(div().flex_1().min_h_0().child(active))
    }
}
