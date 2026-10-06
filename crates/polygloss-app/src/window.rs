//! The one main window (design §11.1, ADR-0023, ADR-0026): the active tab at
//! full size, with no title bar and no tab row. Each page draws the whole
//! shell (sidebar and main column, [`crate::chrome`]); open reviews are
//! listed in the sidebar, and the `Tabs` model and its actions are as
//! before, plus ⌘0 (Home), ⌘1–⌘9 (the open reviews by number, OQ-35) and
//! the sidebar's actions. The window's title follows the active tab. The
//! app keeps running when it closes, and clicking the Dock icon reopens it
//! ([`reopen`], `on_reopen`). Also the native menu bar skeleton (App, File,
//! Edit, View, Review, Window, Help), which feature modules extend with
//! [`add_menu_items`] before [`install_menus`] runs.

use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    InteractiveElement as _, IntoElement, Menu, MenuItem, OsAction, ParentElement as _, Render,
    SharedString, Styled as _, SystemMenuType, WeakEntity, Window, div, px, size,
};

use crate::chrome::{self, Chrome, Segment, ShowFiles, ShowReviews, ToggleSidebar};
use crate::home::HomeView;
use crate::keyboard::{self, Pane};
use crate::keymap::handlers;
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
        /// ⌘0: show Home.
        ShowHome,
        /// ⌘1: the first open review.
        ActivateTab1,
        /// ⌘2: the second open review.
        ActivateTab2,
        /// ⌘3: the third open review.
        ActivateTab3,
        /// ⌘4: the fourth open review.
        ActivateTab4,
        /// ⌘5: the fifth open review.
        ActivateTab5,
        /// ⌘6: the sixth open review.
        ActivateTab6,
        /// ⌘7: the seventh open review.
        ActivateTab7,
        /// ⌘8: the eighth open review.
        ActivateTab8,
        /// ⌘9: the last open review.
        ActivateTab9,
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
    /// The window's title as last set (the active tab's).
    window_title: SharedString,
}

/// Handlers and menu items of the window, its tabs and its sidebar (their
/// key bindings are in `keymap::defaults`).
pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    handlers::on_action(cx, |m: &mut MainWindow, _: &ShowHome, window, cx| {
        m.activate_tab(0, window, cx)
    });
    macro_rules! review_numbers {
        ($($action:ident => $n:literal),* $(,)?) => {$(
            handlers::on_action(cx, |m: &mut MainWindow, _: &$action, window, cx| {
                m.activate_review_number($n, window, cx);
            });
        )*};
    }
    review_numbers![
        ActivateTab1 => 1, ActivateTab2 => 2, ActivateTab3 => 3,
        ActivateTab4 => 4, ActivateTab5 => 5, ActivateTab6 => 6,
        ActivateTab7 => 7, ActivateTab8 => 8, ActivateTab9 => 9,
    ];
    // The sidebar: changed through `Chrome` only, so `MainWindow` hands a
    // hidden tree's keyboard to the diff (`sidebar_changed`).
    handlers::on_action(cx, |_: &mut MainWindow, _: &ToggleSidebar, _, cx| {
        chrome::chrome(cx).update(cx, |c, cx| {
            let visible = c.sidebar_visible();
            c.set_sidebar_visible(!visible, cx)
        });
    });
    handlers::on_action(cx, |_: &mut MainWindow, _: &ShowFiles, window, cx| {
        chrome::show_files(window, cx)
    });
    handlers::on_action(cx, |_: &mut MainWindow, _: &ShowReviews, _, cx| {
        chrome::chrome(cx).update(cx, |c, cx| {
            c.set_sidebar_visible(true, cx);
            c.set_segment(Segment::Reviews, cx);
        });
    });
    add_menu_items(
        MenuKind::App,
        vec![
            // Handled by `editor` (T3.16).
            MenuItem::action("Settings…", crate::keymap::actions::window::OpenSettings),
            // Handled by `install_cli` (T5.4).
            MenuItem::action("Install CLI…", crate::keymap::actions::window::InstallCli),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Polygloss", Quit),
        ],
        cx,
    );
    add_menu_items(
        MenuKind::File,
        vec![MenuItem::action("Close Review", CloseTab)],
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
        vec![
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Threads Panel", ToggleThreadsPanel),
        ],
        cx,
    );
    add_menu_items(
        MenuKind::Window,
        vec![
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Show Next Review", NextTab),
            MenuItem::action("Show Previous Review", PrevTab),
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
    let options = crate::chrome::window_options(Bounds::centered(None, window_size, cx));
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

/// How many times the app asked macOS to bring it forward (a GPUI global).
#[derive(Default)]
struct ActivationRequests(u64);

impl Global for ActivationRequests {}

/// Asks macOS to make the app active. Whether it does is macOS's call
/// (cooperative activation, a locked screen), so this also counts the
/// request for `debug_state` (plan T4.11).
pub fn activate_app(cx: &mut App) {
    cx.default_global::<ActivationRequests>().0 += 1;
    cx.activate(true);
}

/// How many times [`activate_app`] ran.
pub fn activation_requests(cx: &App) -> u64 {
    cx.try_global::<ActivationRequests>().map_or(0, |a| a.0)
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
        let chrome = Chrome::install(cx);
        cx.observe_in(&chrome, window, |this, chrome, window, cx| {
            this.sidebar_changed(&chrome, window, cx)
        })
        .detach();
        // A resize or fullscreen settles running motion (ADR-0030 rule 4).
        cx.observe_window_bounds(window, crate::motion::settle::bounds_observer(window))
            .detach();
        let mut this = MainWindow {
            focus,
            tabs: Tabs::new(home),
            open_errors: Vec::new(),
            toasts: Vec::new(),
            settings_errors_seen: 0,
            window_title: SharedString::default(),
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

    /// Shows `message` as an information toast.
    pub fn toast_info(
        &mut self,
        message: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toasts.push(message.clone());
        window.push_notification(Notification::info(message), cx);
        cx.notify();
    }

    /// Shows `message` as a confirmation toast.
    pub fn toast_success(
        &mut self,
        message: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toasts.push(message.clone());
        window.push_notification(Notification::success(message), cx);
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

    /// The sidebar hid or left Files: if the active review's tree or find
    /// field had the keyboard, the diff takes it before the next frame, so
    /// keys never go where nothing is drawn. Not `on_focus_lost`: GPUI fires
    /// that for anything not drawn, so a composer the wheel scrolled out of
    /// view (the viewport draws only its visible blocks) would lose it too.
    fn sidebar_changed(
        &self,
        chrome: &Entity<Chrome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let TabItem::Review(tab) = self.tabs.active_item() else {
            return;
        };
        if !chrome.read(cx).files_shown()
            && keyboard::focused_pane(tab.read(cx), window, cx) == Some(Pane::Tree)
        {
            self.focus_active(window, cx);
        }
    }

    /// Adds a review tab and activates it; the sidebar switches to its files
    /// (a review opened for the first time, design §11.1).
    pub(crate) fn push_review(
        &mut self,
        tab: Entity<ReviewTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        chrome::chrome(cx).update(cx, |c, cx| c.set_segment(Segment::Files, cx));
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

    /// Activates open review `n` (⌘1–⌘9, OQ-35): 1–8 the Nth open review,
    /// 9 the last. `false`, changing nothing, when there is no such review.
    pub fn activate_review_number(
        &mut self,
        n: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Home is tab 0, so review N is tab N.
        let reviews = self.tabs.len() - 1;
        let ix = match usize::from(n) {
            9 if reviews > 0 => reviews,
            n @ 1..=8 if n <= reviews => n,
            _ => return false,
        };
        self.activate_tab(ix, window, cx);
        true
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
        } else if self.tabs.len() == 1 {
            // Home alone: ⌘W closes the window, as in any Mac app; the app
            // keeps running.
            window.remove_window();
        }
        // Home with reviews open: nothing. Closing the window would drop
        // every review tab (reopening from the Dock brings back Home only);
        // as in Safari, the window closes with its last tab.
    }

    /// Closes tab `ix` (the sidebar's ×), as ⌘W closes the active one.
    pub(crate) fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
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
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.tabs.active_item().title(cx);
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let active: gpui_kit::AnyElement = match self.tabs.active_item() {
            TabItem::Home(home) => home.clone().into_any_element(),
            TabItem::Review(tab) => tab.clone().into_any_element(),
        };
        let root = crate::keymap::handlers::apply(v_flex(), cx);
        let theme = cx.theme();
        root.key_context("Window")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            // First, so its mouse listeners are registered first every frame.
            .child(crate::motion::settle::root())
            .child(div().flex_1().min_h_0().child(active))
    }
}
