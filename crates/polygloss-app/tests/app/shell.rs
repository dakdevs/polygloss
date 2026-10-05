//! The app shell (T3.1, T6.3, design §11.1, §11.7, §5.3, ADR-0026): one
//! window with tabs, Home first, one tab per review; the chrome (no tab row,
//! the sidebar with its top row beside the toolbar row, both 52 pt, the
//! sidebar's width shared and giving way first); the review tab's layout
//! (toolbar, banner strip, file tree | viewport | threads panel), the binary
//! write-back, the launch arguments and reopening after the last window
//! closed.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use futures::FutureExt as _;
use gpui_kit::component::IconName;
use gpui_kit::{
    Bounds, Entity, Modifiers, MouseButton, Pixels, SharedString, Size, TestAppContext,
    VisualTestContext, WindowBackgroundAppearance, WindowBounds, point, px, size,
};
use polygloss_app::chrome::{self, Segment};
use polygloss_app::review_tab::{self, BannerKind, ReviewTab, open_review};
use polygloss_app::startup::{self, LaunchArgs};
use polygloss_app::tabs::TabItem;
use polygloss_app::window::{self, MainWindow};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::{FileKind, ObjectFormat};

use crate::support::{FixtureRepo, Sandbox, code_change_repo, strings};

/// A running app in the sandbox: gpui-kit, every feature, the key bindings
/// and the main window with its Home tab.
pub struct Shell<'a> {
    pub core: Core,
    pub main: Entity<MainWindow>,
    pub cx: &'a mut VisualTestContext,
}

/// Starts the app the way `Polygloss` does (without the platform's run
/// loop). Call after `Sandbox::isolate()`.
pub fn start(cx: &mut TestAppContext) -> Shell<'_> {
    let core = Core::open_default().expect("open the sandbox store");
    let c = core.clone();
    let (window, main) = cx.update(|cx| {
        startup::init(c, cx);
        // Tests never start a real editor (T3.16): `tests/app/editor.rs`
        // installs a recording spawner; anything else fails loudly.
        polygloss_app::editor::set_host(
            polygloss_app::editor::EditorHost::new(Arc::new(NoEditors), || None),
            cx,
        );
        // Nor install the CLI anywhere (T5.4): `tests/app/install_cli.rs`
        // installs a recording installer.
        polygloss_app::install_cli::set_installer(
            polygloss_app::install_cli::CliInstaller::new(
                || None,
                |target: &std::path::Path| panic!("a test tried to install the CLI: {target:?}"),
            ),
            cx,
        );
        window::open_main_window(cx).expect("open the main window")
    });
    let vcx = VisualTestContext::from_window(window, cx).into_mut();
    draw(vcx);
    Shell {
        core,
        main,
        cx: vcx,
    }
}

/// The editor spawner of tests that did not install their own.
struct NoEditors;

impl polygloss_platform::editor::Spawner for NoEditors {
    fn spawn(&self, argv: &[std::ffi::OsString]) -> std::io::Result<()> {
        panic!("a test tried to start an editor: {argv:?}");
    }
}

/// Lets background work finish and draws a few frames.
pub fn draw(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
    }
    cx.run_until_parked();
}

/// `refs/tags/base..refs/tags/head` of [`code_change_repo`]. Full names: on
/// a case-insensitive disk `head` alone is `HEAD`.
pub fn compare_req(repo: &Path) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn commit_req(repo: &Path, rev: &str) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Commit { rev: rev.into() },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

impl Shell<'_> {
    /// Opens `req` through `open_review` and waits for it.
    pub fn open(&mut self, req: OpenRequest) -> anyhow::Result<Entity<ReviewTab>> {
        let task = self.cx.update(|window, cx| open_review(req, window, cx));
        draw(self.cx);
        task.now_or_never().expect("the open finished")
    }

    /// `(tab count, active index)`.
    pub fn tabs(&mut self) -> (usize, usize) {
        self.main
            .read_with(self.cx, |m, _| (m.tabs().len(), m.tabs().active()))
    }

    pub fn tab(&mut self, ix: usize) -> TabItem {
        self.main
            .read_with(self.cx, |m, _| m.tabs().get(ix).cloned())
            .unwrap_or_else(|| panic!("no tab {ix}"))
    }

    pub fn active_review(&mut self) -> Option<Entity<ReviewTab>> {
        let (_, active) = self.tabs();
        self.tab(active).review().cloned()
    }
}

#[gpui_kit::test]
fn home_is_first_tab(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    assert_eq!(shell.tabs(), (1, 0));
    assert!(matches!(shell.tab(0), TabItem::Home(_)));

    shell
        .open(compare_req(repo.path()))
        .expect("open the review");
    assert_eq!(shell.tabs(), (2, 1));
    assert!(matches!(shell.tab(0), TabItem::Home(_)));
    assert!(matches!(shell.tab(1), TabItem::Review(_)));
    // Home cannot be closed; it has no close button.
    let closable = shell.main.read_with(shell.cx, |m, _| {
        (m.tabs().closable(0), m.tabs().closable(1))
    });
    assert_eq!(closable, (false, true));
}

#[gpui_kit::test]
fn opening_same_review_twice_focuses_existing_tab(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let first = shell.open(compare_req(repo.path())).unwrap();
    let other = shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    assert_ne!(first, other);

    let again = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(again, first, "the existing tab, not a new one");
    assert_eq!(shell.tabs(), (3, 1));
    assert_eq!(shell.active_review(), Some(first));
}

#[gpui_kit::test]
fn close_tab_cmd_w(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let first = shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));

    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(shell.active_review(), Some(first));
    // ⌘W on Home while a review is open does nothing: closing the window
    // would lose the review tab.
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    assert_eq!(shell.cx.windows().len(), 1);
    shell.cx.simulate_keystrokes("cmd-}");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1));
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (1, 0));
    // ⌘W on Home alone closes the window (the app keeps running).
    shell.cx.simulate_keystrokes("cmd-w");
    shell.cx.run_until_parked();
    assert!(shell.cx.windows().is_empty());
}

#[gpui_kit::test]
fn next_prev_tab_shortcuts(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    let active = |shell: &mut Shell, keys: &str| {
        shell.cx.simulate_keystrokes(keys);
        draw(shell.cx);
        shell.tabs().1
    };
    // ⌘⇧] / ⌘⇧[ (macOS: ⌘} / ⌘{) and ⌃Tab / ⌃⇧Tab, wrapping around.
    assert_eq!(active(&mut shell, "cmd-}"), 0);
    assert_eq!(active(&mut shell, "cmd-}"), 1);
    assert_eq!(active(&mut shell, "cmd-{"), 0);
    assert_eq!(active(&mut shell, "cmd-{"), 2);
    assert_eq!(active(&mut shell, "ctrl-tab"), 0);
    assert_eq!(active(&mut shell, "ctrl-shift-tab"), 2);
}

#[gpui_kit::test]
fn last_window_closed_app_keeps_running_and_reopens(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    shell.cx.update(|window, _| window.remove_window());
    shell.cx.run_until_parked();
    let (windows, state) = cx.update(|cx| {
        (
            cx.windows().len(),
            cx.has_global::<polygloss_app::app_state::AppState>(),
        )
    });
    assert_eq!(windows, 0);
    assert!(state, "the app and its state live on");
    assert!(cx.update(|cx| window::main_window(cx)).is_none());

    // Clicking the Dock icon (`on_reopen`) opens the window again, Home first.
    cx.update(window::reopen);
    let (_, main) = cx
        .update(|cx| window::main_window(cx))
        .expect("the window is back");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let (len, home) = main.read_with(cx, |m, _| {
        (
            m.tabs().len(),
            matches!(m.tabs().get(0), Some(TabItem::Home(_))),
        )
    });
    assert_eq!((len, home), (1, true));
    // A second reopen with the window open does nothing.
    cx.update(window::reopen);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
}

#[gpui_kit::test]
fn review_tab_has_toolbar_banner_strip_and_three_panes(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let toolbar = bounds(shell.cx, "review-toolbar");
    let banners = bounds(shell.cx, "banner-strip");
    let tree = bounds(shell.cx, "file-tree-pane");
    let viewport = bounds(shell.cx, "viewport-pane");
    let threads = bounds(shell.cx, "threads-pane");
    // The main column, top to bottom: toolbar, banner strip, then the panes.
    assert!(toolbar.bottom() <= banners.top());
    assert!(banners.bottom() <= viewport.top());
    assert!(banners.size.height > px(0.), "reserved height");
    // Left to right: file tree (in the sidebar) | viewport | threads panel.
    assert!(tree.right() <= viewport.left());
    assert!(viewport.right() <= threads.left());
    assert!(viewport.size.width > tree.size.width);
    assert_eq!(threads.top(), viewport.top());

    // The threads panel toggles; the viewport takes its room.
    shell
        .cx
        .dispatch_action(polygloss_app::review_tab::panes::ToggleThreadsPanel);
    draw(shell.cx);
    assert!(!tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    assert!(shell.cx.debug_bounds("threads-pane").is_none());
    let wider = bounds(shell.cx, "viewport-pane");
    assert!(wider.size.width > viewport.size.width);
    shell
        .cx
        .dispatch_action(polygloss_app::review_tab::panes::ToggleThreadsPanel);
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
}

/// The painted bounds of debug selector `name` (any string: leaked, as
/// `debug_bounds` takes `&'static str`).
pub fn bounds(cx: &mut VisualTestContext, name: &str) -> Bounds<Pixels> {
    painted(cx, name).unwrap_or_else(|| panic!("{name} is not painted"))
}

/// The bounds of `name` if it was painted in the last frame.
pub fn painted(cx: &mut VisualTestContext, name: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(name.to_owned().into_boxed_str()))
}

fn window_size(cx: &mut VisualTestContext) -> Size<Pixels> {
    cx.update(|window, _| window.viewport_size())
}

/// Sets the stored sidebar width, as dragging its edge does.
fn set_sidebar_width(shell: &mut Shell, width: f32) {
    let state = shell
        .cx
        .update(|_, cx| chrome::chrome(cx).read(cx).shell().clone());
    shell
        .cx
        .update(|window, cx| state.update(cx, |s, cx| s.resize_panel(0, px(width), window, cx)));
    draw(shell.cx);
}

fn resize_window(shell: &mut Shell, width: f32, height: f32) {
    shell.cx.simulate_resize(size(px(width), px(height)));
    draw(shell.cx);
}

fn click(cx: &mut VisualTestContext, name: &str) {
    let at = bounds(cx, name).center();
    cx.simulate_click(at, Modifiers::none());
    draw(cx);
}

#[test]
fn window_options_inset_the_traffic_lights() {
    let frame = Bounds::new(point(px(40.), px(30.)), size(px(1440.), px(900.)));
    let options = chrome::window_options(frame);
    let titlebar = options.titlebar.expect("titlebar options");
    assert!(titlebar.appears_transparent, "no visible titlebar");
    // macOS's close button is 14 pt; at (19, 19) it is centred in the 52 pt
    // top rows: 19 + 14 / 2 = 26 = 52 / 2.
    assert_eq!(
        titlebar.traffic_light_position,
        Some(point(px(19.), px(19.)))
    );
    assert_eq!(chrome::TOP_ROW_HEIGHT, 52.0);
    assert!(
        options.app_owns_titlebar_drag,
        "our top rows move the window"
    );
    assert_eq!(
        options.window_background,
        WindowBackgroundAppearance::Opaque
    );
    assert_eq!(options.tabbing_identifier, None, "no native window tabs");
    assert_eq!(options.window_min_size, Some(size(px(720.), px(480.))));
    assert_eq!(options.window_bounds, Some(WindowBounds::Windowed(frame)));
    assert!(options.focus && options.show && options.is_movable);
}

#[gpui_kit::test]
fn top_rows_share_the_top_edge_and_height(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    let sidebar_row = bounds(shell.cx, "sidebar-top-row");
    let toolbar = bounds(shell.cx, "review-toolbar");
    for row in [sidebar_row, toolbar] {
        assert_eq!(row.top(), px(0.));
        assert_eq!(row.size.height, px(52.));
    }
    assert_eq!(sidebar_row.left(), px(0.));
    assert!(sidebar_row.right() <= toolbar.left(), "side by side");
    assert_eq!(toolbar.right(), window_size(shell.cx).width);
    // Home has the same two rows.
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    let home_row = bounds(shell.cx, "sidebar-top-row");
    let home_toolbar = bounds(shell.cx, "home-toolbar");
    assert_eq!(home_row, sidebar_row);
    assert_eq!(home_toolbar.top(), px(0.));
    assert_eq!(home_toolbar.size.height, px(52.));
    assert_eq!(home_toolbar.left(), toolbar.left());
}

#[gpui_kit::test]
fn sidebar_spans_the_window_height(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    let window = window_size(shell.cx);
    let sidebar = bounds(shell.cx, "sidebar");
    assert_eq!(sidebar.origin, point(px(0.), px(0.)));
    assert_eq!(sidebar.size.height, window.height);
    assert_eq!(sidebar.size.width, px(280.), "design §11.1's default");
    let main = bounds(shell.cx, "main-column");
    assert_eq!(main.left(), sidebar.right());
    assert_eq!(main.right(), window.width);
    assert_eq!(main.size.height, window.height);
    // The tree fills the sidebar under its top row.
    let tree = bounds(shell.cx, "file-tree-pane");
    assert_eq!(tree.top(), px(52.));
    assert_eq!(tree.bottom(), window.height);
}

#[gpui_kit::test]
fn banner_strip_sits_between_the_toolbar_and_the_viewport(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    let main = bounds(shell.cx, "main-column");
    let toolbar = bounds(shell.cx, "review-toolbar");
    let banners = bounds(shell.cx, "banner-strip");
    let viewport = bounds(shell.cx, "viewport-pane");
    assert_eq!(banners.top(), toolbar.bottom());
    assert_eq!(banners.size.height, px(32.));
    assert_eq!(viewport.top(), banners.bottom());
    // All three span the main column, not the sidebar.
    for b in [toolbar, banners] {
        assert_eq!((b.left(), b.right()), (main.left(), main.right()));
    }
    assert_eq!(viewport.left(), main.left());
}

#[gpui_kit::test]
fn no_tab_bar_is_painted(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    assert!(painted(shell.cx, "tab-bar").is_none(), "Home");
    shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    assert!(painted(shell.cx, "tab-bar").is_none(), "a review");
    // Nothing sits above the top rows.
    assert_eq!(bounds(shell.cx, "sidebar").top(), px(0.));
    assert_eq!(bounds(shell.cx, "main-column").top(), px(0.));
}

#[gpui_kit::test]
fn window_title_follows_the_active_item(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    assert_eq!(shell.cx.window_title().as_deref(), Some("Home"));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let name = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let expected = format!("{name} · base..head");
    assert_eq!(tab.read_with(shell.cx, |t, _| t.title()), expected);
    assert_eq!(shell.cx.window_title(), Some(expected));
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.cx.window_title().as_deref(), Some("Home"));
}

#[gpui_kit::test]
fn segment_control_sits_right_of_the_traffic_light_inset(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    for width in [280., 220.] {
        set_sidebar_width(&mut shell, width);
        let sidebar = bounds(shell.cx, "sidebar");
        assert_eq!(sidebar.size.width, px(width));
        let row = bounds(shell.cx, "sidebar-top-row");
        let files = bounds(shell.cx, "segment-files");
        let reviews = bounds(shell.cx, "segment-reviews");
        let toggle = bounds(shell.cx, "toggle-sidebar");
        assert!(files.left() >= px(80.), "{width}: {files:?}");
        assert!(files.right() <= reviews.left(), "{width}");
        assert!(reviews.right() <= toggle.left(), "{width}");
        assert!(toggle.right() <= sidebar.right(), "{width}");
        for b in [files, reviews, toggle] {
            assert!(b.top() > row.top() && b.bottom() < row.bottom(), "{width}");
        }
    }
}

#[gpui_kit::test]
fn hidden_sidebar_insets_the_toolbar_and_shows_the_open_button(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    set_sidebar_width(&mut shell, 300.);
    assert!(painted(shell.cx, "show-sidebar").is_none());
    click(shell.cx, "toggle-sidebar");
    let visible = |shell: &mut Shell| {
        shell
            .cx
            .update(|_, cx| chrome::chrome(cx).read(cx).sidebar_visible())
    };
    assert!(!visible(&mut shell));
    assert!(painted(shell.cx, "sidebar").is_none());
    assert!(painted(shell.cx, "sidebar-top-row").is_none());
    let window = window_size(shell.cx);
    let toolbar = bounds(shell.cx, "review-toolbar");
    assert_eq!((toolbar.left(), toolbar.right()), (px(0.), window.width));
    assert_eq!(bounds(shell.cx, "main-column").left(), px(0.));
    // The toolbar keeps the traffic lights clear, then offers the sidebar.
    let open = bounds(shell.cx, "show-sidebar");
    assert!(open.left() >= px(80.), "{open:?}");
    assert!(open.left() < px(140.), "first in the row: {open:?}");

    // In fullscreen the lights are AppKit's again and the inset goes.
    shell.cx.update(|window, _| window.toggle_fullscreen());
    draw(shell.cx);
    let open_fullscreen = bounds(shell.cx, "show-sidebar");
    assert!(open_fullscreen.left() < px(80.), "{open_fullscreen:?}");
    shell.cx.update(|window, _| window.toggle_fullscreen());
    draw(shell.cx);

    click(shell.cx, "show-sidebar");
    assert!(visible(&mut shell));
    assert_eq!(bounds(shell.cx, "sidebar").size.width, px(300.), "kept");
    assert!(painted(shell.cx, "show-sidebar").is_none());
}

#[gpui_kit::test]
fn sidebar_width_is_shared_by_every_review(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    set_sidebar_width(&mut shell, 330.);
    assert_eq!(bounds(shell.cx, "sidebar").size.width, px(330.));
    for (keys, active) in [("cmd-{", 1), ("cmd-{", 0), ("cmd-{", 2)] {
        shell.cx.simulate_keystrokes(keys);
        draw(shell.cx);
        assert_eq!(shell.tabs().1, active);
        assert_eq!(
            bounds(shell.cx, "sidebar").size.width,
            px(330.),
            "tab {active}"
        );
    }
}

#[gpui_kit::test]
fn sidebar_gives_way_to_the_main_column_minimum(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let sidebar = |shell: &mut Shell| bounds(shell.cx, "sidebar").size.width;
    resize_window(&mut shell, 1440., 900.);
    set_sidebar_width(&mut shell, 480.);
    assert_eq!(sidebar(&mut shell), px(480.));
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));

    // 720 pt: the threads panel shows, so the main column keeps 480 pt
    // (viewport 260 + panel 220) and the sidebar gets 720 − 480.
    resize_window(&mut shell, 720., 480.);
    assert_eq!(sidebar(&mut shell), px(240.));
    // Without the panel the main column needs 320 pt: 720 − 320.
    tab.update(shell.cx, |t, cx| t.toggle_threads_panel(cx));
    draw(shell.cx);
    assert_eq!(sidebar(&mut shell), px(400.));
    let main = bounds(shell.cx, "main-column");
    assert_eq!(main.size.width, px(320.));
    assert_eq!(main.right(), px(720.), "nothing past the window's edge");
    // The stored width comes back when the window widens.
    resize_window(&mut shell, 1440., 900.);
    assert_eq!(sidebar(&mut shell), px(480.));
    tab.update(shell.cx, |t, cx| t.toggle_threads_panel(cx));
    draw(shell.cx);
    assert_eq!(sidebar(&mut shell), px(480.));
}

#[gpui_kit::test]
fn top_row_buttons_never_move_the_window(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    // A press on a button followed by a drag stays the button's: the test
    // platform panics on `start_window_move`, so a row that took the press
    // would fail this test.
    for name in ["toggle-threads-panel", "segment-reviews", "toggle-sidebar"] {
        let at = bounds(shell.cx, name).center();
        shell
            .cx
            .simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        shell.cx.simulate_mouse_move(
            point(at.x + px(6.), at.y + px(3.)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        shell
            .cx
            .simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        draw(shell.cx);
    }
}

#[gpui_kit::test]
#[should_panic(expected = "not implemented")]
fn dragging_a_top_row_moves_the_window(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    // Empty toolbar space between the two clusters. The test platform's
    // `start_window_move` is `unimplemented!()`, so reaching it panics.
    let toolbar = bounds(shell.cx, "review-toolbar");
    let at = point(toolbar.center().x, toolbar.center().y);
    shell
        .cx
        .simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    shell.cx.simulate_mouse_move(
        point(at.x + px(6.), at.y),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
}

#[gpui_kit::test]
fn nav_stub_lists_open_reviews_and_closes_like_cmd_w(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let first = shell.open(compare_req(repo.path())).unwrap();
    let second = shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    let ids: Vec<String> = [&first, &second]
        .iter()
        .map(|t| t.read_with(shell.cx, |t, _| t.review_id.clone()))
        .collect();
    click(shell.cx, "segment-reviews");
    assert_eq!(
        shell
            .cx
            .update(|_, cx| chrome::chrome(cx).read(cx).segment()),
        Segment::Reviews
    );
    assert!(painted(shell.cx, "file-tree-pane").is_none());
    let home = bounds(shell.cx, "nav-home");
    let row = |id: &str| format!("open-review-{id}");
    let (a, b) = (
        bounds(shell.cx, &row(&ids[0])),
        bounds(shell.cx, &row(&ids[1])),
    );
    assert!(
        home.bottom() <= a.top() && a.bottom() <= b.top(),
        "in tab order"
    );

    // A click activates the row's review; Home's row activates Home.
    click(shell.cx, &row(&ids[0]));
    assert_eq!(shell.tabs(), (3, 1));
    click(shell.cx, "nav-home");
    assert_eq!(shell.tabs(), (3, 0));
    click(shell.cx, &row(&ids[1]));
    assert_eq!(shell.tabs(), (3, 2));

    // × closes its review as ⌘W does: the tab to its left becomes active.
    click(shell.cx, &format!("close-review-{}", ids[1]));
    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(shell.active_review(), Some(first.clone()));
    assert!(painted(shell.cx, &row(&ids[1])).is_none());
    // Closing a review that is not the active one keeps the active one.
    click(shell.cx, "nav-home");
    click(shell.cx, &format!("close-review-{}", ids[0]));
    assert_eq!(shell.tabs(), (1, 0));
    assert!(painted(shell.cx, "nav-home").is_some());
}

#[gpui_kit::test]
fn home_renders_in_the_shell_with_files_disabled(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    assert_eq!(shell.tabs(), (1, 0));
    let window = window_size(shell.cx);
    let sidebar = bounds(shell.cx, "sidebar");
    assert_eq!(sidebar.size.height, window.height);
    assert_eq!(bounds(shell.cx, "main-column").left(), sidebar.right());
    assert!(painted(shell.cx, "home-toolbar").is_some());
    // Reviews shows on Home whatever the window's segment is (Files by
    // default), and Files cannot be picked.
    let segment = |shell: &mut Shell| {
        shell
            .cx
            .update(|_, cx| chrome::chrome(cx).read(cx).segment())
    };
    assert_eq!(segment(&mut shell), Segment::Files);
    assert!(painted(shell.cx, "nav-home").is_some());
    assert!(painted(shell.cx, "file-tree-pane").is_none());
    click(shell.cx, "segment-files");
    assert!(painted(shell.cx, "nav-home").is_some());
    assert_eq!(segment(&mut shell), Segment::Files, "unchanged");
}

#[test]
fn every_app_icon_loads_from_app_assets() {
    use gpui_kit::AssetSource as _;
    use gpui_kit::assets::IconName as Lucide;
    use polygloss_app::assets::{AppAssets, AppIcons};
    // Plan T6.3's list, by hand.
    let listed = [
        Lucide::ListTree,
        Lucide::RotateCcwClock,
        Lucide::PanelLeft,
        Lucide::PanelLeftOpen,
        Lucide::GitBranch,
        Lucide::GitCommitHorizontal,
        Lucide::GitCompare,
        Lucide::CircleDot,
        Lucide::MessageSquare,
        Lucide::MessageSquareDot,
        Lucide::Columns2,
        Lucide::Rows2,
        Lucide::SlidersHorizontal,
        Lucide::SquareArrowOutUpRight,
        Lucide::Square,
        Lucide::SquareCheck,
        Lucide::Circle,
        Lucide::CircleCheck,
        Lucide::CircleMinus,
        Lucide::Camera,
        Lucide::FlaskConical,
        Lucide::FileCog,
        Lucide::Package,
        Lucide::BookOpen,
        Lucide::Wrench,
        Lucide::Layers,
        Lucide::Languages,
        Lucide::Tag,
        Lucide::FileSymlink,
        Lucide::Binary,
        Lucide::HardDrive,
        Lucide::FolderGit,
        Lucide::Dot,
    ];
    for icon in listed {
        let path = icon.path();
        let svg = AppAssets
            .load(&path)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
            .unwrap_or_else(|| panic!("{path} is not in AppAssets"));
        assert!(svg.starts_with(b"<svg"), "{path}");
    }
    assert_eq!(
        AppIcons.list("icons/").unwrap().len(),
        listed.len(),
        "AppIcons holds the listed icons"
    );
    // gpui-kit's own icons still load, behind AppIcons.
    for icon in [
        IconName::Close,
        IconName::Search,
        IconName::Inbox,
        IconName::ChevronDown,
    ] {
        let path = gpui_kit::assets::IconName::from(icon).path();
        assert!(AppAssets.load(&path).unwrap().is_some(), "{path}");
    }
    // An icon neither registers draws nothing.
    let rocket = Lucide::Rocket.path();
    assert!(!matches!(AppAssets.load(&rocket), Ok(Some(_))), "{rocket}");
}
#[gpui_kit::test]
fn banner_strip_never_changes_viewport_anchor(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| v.scroll_by(90.0, cx));
    draw(shell.cx);
    let before = (
        viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows)),
        shell.cx.debug_bounds("viewport-pane"),
    );

    let banners = tab.read_with(shell.cx, |t, _| t.banners.clone());
    banners.update(shell.cx, |b, cx| {
        b.set(
            BannerKind::LiveChanges,
            "3 files changed".into(),
            Box::new(polygloss_app::review_tab::panes::ToggleThreadsPanel),
            cx,
        );
        b.set(
            BannerKind::AgentReplies,
            "claude-code replied to 2 threads".into(),
            Box::new(polygloss_app::review_tab::panes::ToggleThreadsPanel),
            cx,
        );
    });
    draw(shell.cx);
    let shown: Vec<(BannerKind, SharedString)> = banners.read_with(shell.cx, |b, _| b.banners());
    assert_eq!(
        shown,
        [
            (BannerKind::LiveChanges, "3 files changed".into()),
            (
                BannerKind::AgentReplies,
                "claude-code replied to 2 threads".into()
            ),
        ]
    );
    let with_banners = (
        viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows)),
        shell.cx.debug_bounds("viewport-pane"),
    );
    assert_eq!(with_banners, before, "nothing moved");

    banners.update(shell.cx, |b, cx| {
        b.clear(BannerKind::LiveChanges, cx);
        b.clear(BannerKind::AgentReplies, cx);
    });
    draw(shell.cx);
    assert!(banners.read_with(shell.cx, |b, _| b.banners()).is_empty());
    let cleared = (
        viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows)),
        shell.cx.debug_bounds("viewport-pane"),
    );
    assert_eq!(cleared, before);
}

#[gpui_kit::test]
fn toolbar_and_banner_buttons_reach_the_tab_without_focus(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let visible = |shell: &mut Shell| tab.read_with(shell.cx, |t, _| t.threads_panel_visible());
    let click = |shell: &mut Shell, name: &'static str| {
        // Nothing focused: an action dispatched from the focused element
        // would never reach the tab.
        shell.cx.update(|window, cx| window.blur(cx));
        let at = shell
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not painted"))
            .center();
        shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
        draw(shell.cx);
    };
    assert!(visible(&mut shell));
    click(&mut shell, "toggle-threads-panel");
    assert!(!visible(&mut shell), "the toolbar toggle hid the panel");

    let banners = tab.read_with(shell.cx, |t, _| t.banners.clone());
    banners.update(shell.cx, |b, cx| {
        b.set(
            BannerKind::NewIteration,
            "New iteration available".into(),
            Box::new(polygloss_app::review_tab::panes::ToggleThreadsPanel),
            cx,
        );
    });
    draw(shell.cx);
    click(&mut shell, "banner-button-0");
    assert!(visible(&mut shell), "the banner's action reached the tab");
}

#[gpui_kit::test]
fn binary_detected_writes_file_kind_back(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("data.dat", b"text for now\n");
    repo.write("notes.txt", b"one\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    // No attribute says binary, so `diff-tree` lists it as text; the NUL is
    // found when the viewport reads the blob.
    repo.write("data.dat", b"bin\0ary\n");
    repo.write("notes.txt", b"two\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    let idx = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .position(|f| f.display_path() == "data.dat")
            .unwrap()
    });
    let kind = |core: &Core| core.files_for_diff(&diff_id).unwrap().unwrap()[idx].kind;
    draw(shell.cx);
    assert_eq!(kind(&shell.core), FileKind::Binary);
    assert_eq!(
        shell
            .core
            .files_for_diff(&diff_id)
            .unwrap()
            .unwrap()
            .iter()
            .filter(|f| f.kind == FileKind::Binary)
            .count(),
        1
    );
    // The next open sees it as binary.
    let reopened = shell.core.open(&compare_req(repo.path())).unwrap();
    assert_eq!(reopened.files[idx].kind, FileKind::Binary);
}

#[gpui_kit::test]
fn missing_objects_show_no_longer_available(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    // A review opened earlier, whose blobs are gone since (design §5.3: commit
    // and compare iterations rely on the user's objects).
    // A shallow clone for the second half, made while every object exists.
    let shallow = repo.clone_shallow();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    for f in opened.files.iter() {
        for oid in [&f.old_blob, &f.new_blob] {
            let hex = oid.to_string();
            let _ = std::fs::remove_file(
                repo.path()
                    .join(".git/objects")
                    .join(&hex[..2])
                    .join(&hex[2..]),
            );
        }
    }
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let rows = viewport.read_with(shell.cx, |v, _| v.debug().visible_rows);
    assert!(
        rows.iter()
            .any(|r| r.contains("objects no longer available")),
        "{rows:#?}"
    );

    // Objects missing at open (history beyond a shallow clone): the open
    // fails with the same wording, shown in the window, and no tab opens.
    let before = shell.tabs().0;
    let err = shell
        .open(commit_req(shallow.path(), "HEAD"))
        .expect_err("the parent is not in the clone");
    assert!(
        err.to_string().contains("objects no longer available"),
        "{err:#}"
    );
    assert_eq!(shell.tabs().0, before);
    let shown = shell
        .main
        .read_with(shell.cx, |m, _| m.open_errors().to_vec());
    assert!(
        shown
            .last()
            .is_some_and(|m| m.contains("objects no longer available")),
        "{shown:?}"
    );
}

#[test]
fn launch_args_parse_every_source() {
    let parse = |args: &[&str]| LaunchArgs::parse(&strings(args));
    assert!(parse(&[]).unwrap().open.is_none());
    let args = parse(&["--repo", "/r", "--compare", "main", "topic", "--direct"]).unwrap();
    let req = args.open.expect("a review to open");
    assert_eq!(req.worktree, Path::new("/r"));
    assert_eq!(
        req.source,
        Source::Compare {
            base: "main".into(),
            head: "topic".into(),
            mode: CompareMode::Direct,
        }
    );
    let req = parse(&["--repo", "/r", "--live"]).unwrap().open.unwrap();
    assert_eq!(
        req.source,
        Source::Live {
            since: Since::MergeBase
        }
    );
    let req = parse(&["--commit", "HEAD~1", "--repo", "/r"])
        .unwrap()
        .open
        .unwrap();
    assert_eq!(
        req.source,
        Source::Commit {
            rev: "HEAD~1".into()
        }
    );
    for bad in [
        &["--repo", "/r"][..],
        &["--live"],
        &["--repo", "/r", "--live", "--commit", "x"],
        &["--repo", "/r", "--commit", "x", "--direct"],
        &["--frobnicate"],
        &["--gate", "--repo", "/r", "--live"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

fn polygloss() -> Command {
    Command::new(env!("CARGO_BIN_EXE_Polygloss"))
}

#[test]
fn usage_errors_exit_2_without_a_window() {
    let _sb = Sandbox::isolate();
    for args in [&["--repo", "/r"][..], &["--frobnicate"], &["--gate"]] {
        let out = polygloss().args(args).output().expect("run Polygloss");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("usage: Polygloss"), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn perf_scenarios_need_polygloss_test() {
    let _sb = Sandbox::isolate();
    let out = polygloss()
        .env_remove("POLYGLOSS_TEST")
        .args(["--perf-scenario", "open", "--corpus", "typical", "--json"])
        .output()
        .expect("run Polygloss");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("POLYGLOSS_TEST=1"), "{stderr}");
    // With it, bad scenario arguments are usage errors too (no window).
    let out = polygloss()
        .env("POLYGLOSS_TEST", "1")
        .args(["--perf-scenario", "nope", "--corpus", "typical"])
        .output()
        .expect("run Polygloss");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn logging_writes_a_rolling_file_in_the_logs_dir() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("Library/Logs/polygloss");
    let guard = polygloss_app::logging::init(&dir).expect("logging starts");
    tracing::info!(target: "polygloss_app", "hello from the log test");
    tracing::debug!(target: "polygloss_app", "below the default level");
    drop(guard);
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("polygloss.") && name.ends_with(".log"),
        "{name}"
    );
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("hello from the log test"), "{text}");
    assert!(!text.contains("below the default level"), "{text}");
}

#[test]
fn review_titles_name_the_repo_and_both_sides() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    repo.git(&["branch", "topic", "refs/tags/head"]);
    repo.git(&["branch", "trunk", "refs/tags/base"]);
    let core = Core::open_default().unwrap();
    let name = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut req = compare_req(repo.path());
    req.source = Source::Compare {
        base: "trunk".into(),
        head: "topic".into(),
        mode: CompareMode::ThreeDot,
    };
    let opened = core.open(&req).unwrap();
    assert_eq!(
        review_tab::title(&opened),
        format!("{name} · trunk...topic")
    );
    let base = opened.base.commit.as_ref().unwrap().short().to_string();
    let head = opened.head_commit.as_ref().unwrap().short().to_string();
    assert_eq!(
        review_tab::description(&opened),
        format!("trunk ({base}) → topic ({head}) · 3 files changed")
    );
    let commit = core.open(&commit_req(repo.path(), "topic")).unwrap();
    assert_eq!(review_tab::title(&commit), format!("{name} · {head}"));

    // Dotted refs (version tags, release branches) keep their dots: the key
    // splits at the separator, not at the last dot.
    repo.git(&["tag", "v1.2.0", "refs/tags/head"]);
    repo.git(&["branch", "release-1.2", "refs/tags/head"]);
    for (head_ref, mode, sep, shown) in [
        ("refs/tags/v1.2.0", CompareMode::ThreeDot, "...", "v1.2.0"),
        ("v1.2.0", CompareMode::Direct, "..", "v1.2.0"),
        ("release-1.2", CompareMode::Direct, "..", "release-1.2"),
    ] {
        req.source = Source::Compare {
            base: "trunk".into(),
            head: head_ref.into(),
            mode,
        };
        let opened = core.open(&req).unwrap();
        assert_eq!(
            review_tab::title(&opened),
            format!("{name} · trunk{sep}{shown}"),
            "{head_ref}"
        );
        assert_eq!(
            review_tab::description(&opened),
            format!("trunk ({base}) → {shown} ({head}) · 3 files changed"),
            "{head_ref}"
        );
    }

    // Live keys: branch names may hold `@` and `#`.
    repo.git(&["checkout", "-q", "-b", "feat@x#1", "refs/tags/head"]);
    let live = core
        .open(&OpenRequest {
            source: Source::Live { since: Since::Head },
            ..compare_req(repo.path())
        })
        .unwrap();
    assert_eq!(
        review_tab::title(&live),
        format!("{name} · feat@x#1 (working tree)")
    );
    assert_eq!(review_tab::short_ref(&"a".repeat(40)), "aaaaaaa");
    assert_eq!(
        review_tab::short_ref("refs/remotes/origin/main"),
        "origin/main"
    );
}
