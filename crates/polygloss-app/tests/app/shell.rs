//! The app shell (T3.1, T6.3, design §11.1, §11.7, §5.3, ADR-0026): one
//! window with tabs, Home first, one tab per review; the chrome (no tab row,
//! the sidebar with its top row beside the toolbar row, both 52 pt, the
//! sidebar's width shared and giving way first); the review tab's layout
//! (toolbar, banner strip, file tree | viewport | threads panel), the binary
//! write-back, the launch arguments and reopening after the last window
//! closed.

use std::cell::RefCell;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;

use futures::FutureExt as _;
use gpui_kit::component::IconName;
use gpui_kit::{
    Bounds, Entity, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels,
    PlatformInput, Point, SharedString, Size, TestAppContext, VisualTestContext,
    WindowBackgroundAppearance, WindowBounds, point, px, size,
};
use polygloss_app::chrome::{self, Segment, TitlebarGesture};
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

pub fn commit_req(repo: &Path, rev: &str) -> OpenRequest {
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
pub fn set_sidebar_width(shell: &mut Shell, width: f32) {
    let state = shell
        .cx
        .update(|_, cx| chrome::chrome(cx).read(cx).shell().clone());
    shell
        .cx
        .update(|window, cx| state.update(cx, |s, cx| s.resize_panel(0, px(width), window, cx)));
    draw(shell.cx);
}

pub fn resize_window(shell: &mut Shell, width: f32, height: f32) {
    shell.cx.simulate_resize(size(px(width), px(height)));
    draw(shell.cx);
}

pub fn click(cx: &mut VisualTestContext, name: &str) {
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

/// Asserts that `columns` sit side by side across the whole window and that
/// each one's `rows` stack from its top to the window's bottom with nothing
/// between them.
fn assert_tiles(cx: &mut VisualTestContext, columns: [(&str, &[&str]); 2]) {
    let window = window_size(cx);
    let mut left = px(0.);
    for (column, rows) in columns {
        let c = bounds(cx, column);
        assert_eq!((c.left(), c.top()), (left, px(0.)), "{column}");
        assert_eq!(c.bottom(), window.height, "{column}");
        left = c.right();
        let mut top = px(0.);
        for row in rows {
            let r = bounds(cx, row);
            assert_eq!(r.top(), top, "{row} in {column}");
            assert_eq!(r.left(), c.left(), "{row} in {column}");
            top = r.bottom();
        }
        assert_eq!(top, window.height, "{column} ends with {rows:?}");
    }
    assert_eq!(left, window.width);
}

#[gpui_kit::test]
fn no_tab_bar_is_painted(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    // Each column is its known rows, top to bottom: no room for a tab row
    // above, between or below them.
    assert_tiles(
        shell.cx,
        [
            ("sidebar", &["sidebar-top-row", "file-tree-pane"]),
            (
                "main-column",
                &["review-toolbar", "banner-strip", "viewport-pane"],
            ),
        ],
    );
    shell.cx.simulate_keystrokes("cmd-{ cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (3, 0));
    let TabItem::Home(home) = shell.tab(0) else {
        panic!("tab 0 is Home")
    };
    home.update(shell.cx, |h, cx| h.refresh(cx));
    draw(shell.cx);
    assert_tiles(
        shell.cx,
        [
            ("sidebar", &["sidebar-top-row", "nav"]),
            ("main-column", &["home-toolbar", "home-list"]),
        ],
    );
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

/// The top rows' gestures, recorded (the test platform can neither move nor
/// zoom a window).
fn record_gestures(cx: &mut VisualTestContext) -> Rc<RefCell<Vec<TitlebarGesture>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    cx.update(|_, cx| chrome::redirect_titlebar_gestures(move |g| sink.borrow_mut().push(g), cx));
    log
}

/// Presses along the middle of top row `row` (every control's centre, every
/// gap between controls and every 30 pt, 2 pt clear of control edges and
/// 8 pt clear of the row's ends, where the sidebar's resize handle takes
/// presses within 5 pt of its edge):
/// a drag must move the window and a double click zoom it, except on the
/// painted controls among `controls`, where neither may happen. Returns how
/// many points were on a control and how many off.
fn sweep_top_row(
    cx: &mut VisualTestContext,
    log: &Rc<RefCell<Vec<TitlebarGesture>>>,
    row: &str,
    controls: &[&str],
) -> (usize, usize) {
    let r = bounds(cx, row);
    let mut controls: Vec<Bounds<Pixels>> =
        controls.iter().filter_map(|c| painted(cx, c)).collect();
    controls.sort_by(|a, b| a.left().as_f32().total_cmp(&b.left().as_f32()));
    let mut xs: Vec<Pixels> = controls.iter().map(|c| c.center().x).collect();
    xs.extend(
        controls
            .windows(2)
            .map(|w| (w[0].right() + w[1].left()) / 2.),
    );
    let mut x = r.left() + px(8.);
    while x < r.right() - px(8.) {
        xs.push(x);
        x += px(30.);
    }
    let away = point(px(-50.), px(-50.));
    let (mut on, mut off) = (0, 0);
    for x in xs {
        let at = point(x, r.center().y);
        if x < r.left() + px(8.)
            || x > r.right() - px(8.)
            || controls
                .iter()
                .any(|c| (x - c.left()).abs() < px(2.) || (x - c.right()).abs() < px(2.))
        {
            continue;
        }
        let on_control = controls.iter().any(|c| c.contains(&at));
        log.borrow_mut().clear();
        let press = |cx: &mut VisualTestContext, click_count| {
            cx.simulate_event(MouseDownEvent {
                position: at,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
            })
        };
        // On a control, every press is released off the window, so it
        // clicks nothing.
        let release = |cx: &mut VisualTestContext, click_count| {
            cx.simulate_event(MouseUpEvent {
                position: if on_control { away } else { at },
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
            })
        };
        press(cx, 1);
        cx.simulate_mouse_move(
            point(at.x + px(6.), at.y),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        release(cx, 1);
        press(cx, 2);
        release(cx, 2);
        let want = if on_control {
            on += 1;
            vec![]
        } else {
            off += 1;
            vec![TitlebarGesture::Move, TitlebarGesture::DoubleClick]
        };
        assert_eq!(*log.borrow(), want, "{row} at x {x:?}");
    }
    (on, off)
}

/// Every toolbar control that may be painted in the review toolbar: the
/// buttons and the pills (design §11.4).
const TOOLBAR_CONTROLS: [&str; 13] = [
    "show-sidebar",
    "commit-pill",
    "ref-pill-base",
    "ref-pill-head",
    "branch-pill",
    "live-base",
    "live-snapshot",
    "iteration-picker",
    "toolbar-find",
    "toggle-threads-panel",
    "layout-toggle",
    "view-options",
    "submit-review",
];

#[gpui_kit::test]
fn top_rows_move_the_window_except_on_controls(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let log = record_gestures(shell.cx);
    let segments = ["sidebar-segments", "toggle-sidebar"];
    // The repo block, the compare mode, the progress and the gaps drag like
    // the empty room; the buttons, the pills and the segmented controls
    // (rims included) never.
    let (on, off) = sweep_top_row(shell.cx, &log, "review-toolbar", &TOOLBAR_CONTROLS);
    assert!(on >= 4 && off >= 10, "{on} on controls, {off} off");
    let (on, off) = sweep_top_row(shell.cx, &log, "sidebar-top-row", &segments);
    assert!(on >= 2 && off >= 3, "{on} on controls, {off} off");
    // The keyboard stayed in the diff throughout.
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    assert!(shell.cx.update(|window, _| viewport.is_focused(window)));

    // Hidden sidebar: the traffic lights' room and the show button.
    click(shell.cx, "toggle-sidebar");
    let (on, _) = sweep_top_row(shell.cx, &log, "review-toolbar", &TOOLBAR_CONTROLS);
    assert!(on >= 5, "show-sidebar is among {on} controls");
    click(shell.cx, "show-sidebar");

    // Home: its title and count drag too.
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    let (on, off) = sweep_top_row(shell.cx, &log, "home-toolbar", &[]);
    assert!(on == 0 && off >= 10, "{on} on controls, {off} off");
    sweep_top_row(shell.cx, &log, "sidebar-top-row", &segments);
}

/// Moves the pointer onto `name` and draws, so what shows on hover (a nav
/// row's ×) is painted and clickable.
pub fn hover(cx: &mut VisualTestContext, name: &str) {
    let at = bounds(cx, name).center();
    cx.simulate_mouse_move(at, None, Modifiers::none());
    draw(cx);
}

/// Moves the pointer off every row (the window's top-left corner, in the
/// sidebar's top row).
pub fn unhover(cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(point(px(1.), px(1.)), None, Modifiers::none());
    draw(cx);
}

#[gpui_kit::test]
fn nav_close_closes_like_cmd_w(cx: &mut TestAppContext) {
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
    let row = |id: &str| format!("open-review-{id}");
    let close = |id: &str| format!("close-review-{id}");

    // At rest the × is hidden: a press where it shows on hover lands on the
    // row, which activates its review and closes nothing.
    click(shell.cx, "nav-home");
    hover(shell.cx, &row(&ids[0]));
    let at = bounds(shell.cx, &close(&ids[0])).center();
    unhover(shell.cx);
    assert!(painted(shell.cx, &close(&ids[0])).is_none(), "hidden");
    shell.cx.simulate_click(at, Modifiers::none());
    draw(shell.cx);
    assert_eq!(shell.tabs(), (3, 1), "the row activated");

    // On hover, × closes its review as ⌘W does: the tab to its left
    // becomes active.
    click(shell.cx, &row(&ids[1]));
    assert_eq!(shell.tabs(), (3, 2));
    hover(shell.cx, &row(&ids[1]));
    click(shell.cx, &close(&ids[1]));
    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(shell.active_review(), Some(first.clone()));
    assert!(painted(shell.cx, &row(&ids[1])).is_none());
    // Closing a review that is not the active one keeps the active one.
    click(shell.cx, "nav-home");
    hover(shell.cx, &row(&ids[0]));
    click(shell.cx, &close(&ids[0]));
    assert_eq!(shell.tabs(), (1, 0));
    assert!(painted(shell.cx, "nav-home").is_some());
}

/// ⌘W, then a click at `at` before the window draws again (both reach the
/// last frame's handlers, as input queued within one frame does).
fn close_then_click(shell: &mut Shell, at: Point<Pixels>) {
    shell.cx.update(|window, cx| {
        window.dispatch_keystroke(Keystroke::parse("cmd-w").unwrap(), cx);
        let (position, modifiers, button) = (at, Modifiers::none(), MouseButton::Left);
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                position,
                modifiers,
                button,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position,
                modifiers,
                button,
                click_count: 1,
            }),
            cx,
        );
    });
    draw(shell.cx);
}

#[gpui_kit::test]
fn nav_clicks_act_on_their_review_after_a_close(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let a = shell.open(compare_req(repo.path())).unwrap();
    let b = shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    let id = |shell: &mut Shell, t: &Entity<ReviewTab>| {
        t.read_with(shell.cx, |t, _| t.review_id.clone())
    };
    let (a_id, b_id) = (id(&mut shell, &a), id(&mut shell, &b));
    click(shell.cx, "segment-reviews");
    click(shell.cx, &format!("open-review-{a_id}"));
    assert_eq!(shell.tabs(), (3, 1));

    // ⌘W closes A, then B's row is clicked where the last frame drew it
    // (its tab was third there, second now): B activates.
    let b_row = bounds(shell.cx, &format!("open-review-{b_id}")).center();
    close_then_click(&mut shell, b_row);
    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(shell.active_review(), Some(b.clone()));

    // A again, after B; B active. ⌘W closes B, then B's × is clicked where
    // it was (second, where A is now): A stays open. (A first open shows
    // its files: back to Reviews.)
    let a = shell.open(compare_req(repo.path())).unwrap();
    click(shell.cx, "segment-reviews");
    click(shell.cx, &format!("open-review-{b_id}"));
    assert_eq!(shell.tabs(), (3, 1));
    hover(shell.cx, &format!("open-review-{b_id}"));
    let b_close = bounds(shell.cx, &format!("close-review-{b_id}")).center();
    close_then_click(&mut shell, b_close);
    assert_eq!(shell.tabs(), (2, 0));
    assert_eq!(shell.tab(1).review(), Some(&a));
}

#[gpui_kit::test]
fn sidebar_state_lasts_for_the_session(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    set_sidebar_width(&mut shell, 330.);
    let chrome = shell.cx.update(|_, cx| chrome::chrome(cx));
    chrome.update(shell.cx, |c, cx| c.set_segment(Segment::Reviews, cx));
    shell.cx.update(|window, _| window.remove_window());
    shell.cx.run_until_parked();

    // Reopened from the Dock: the same segment and width.
    let reopen = |cx: &mut TestAppContext| {
        cx.update(window::reopen);
        let (window, _) = cx.update(|cx| window::main_window(cx)).expect("reopened");
        let vcx = VisualTestContext::from_window(window, cx).into_mut();
        draw(vcx);
        vcx
    };
    let vcx = reopen(cx);
    let (segment, visible) = vcx.update(|_, cx| {
        let c = chrome::chrome(cx).read(cx);
        (c.segment(), c.sidebar_visible())
    });
    assert_eq!((segment, visible), (Segment::Reviews, true));
    assert_eq!(bounds(vcx, "sidebar").size.width, px(330.));

    // Hidden stays hidden.
    click(vcx, "toggle-sidebar");
    vcx.update(|window, _| window.remove_window());
    vcx.run_until_parked();
    let vcx = reopen(cx);
    assert!(painted(vcx, "sidebar").is_none());
    click(vcx, "show-sidebar");
    assert_eq!(bounds(vcx, "sidebar").size.width, px(330.));
}

/// `(sidebar visible, segment)`.
fn sidebar_state(shell: &mut Shell) -> (bool, Segment) {
    shell.cx.update(|_, cx| {
        let c = chrome::chrome(cx).read(cx);
        (c.sidebar_visible(), c.segment())
    })
}

/// Presses `keys` and returns the active tab's index.
fn press(shell: &mut Shell, keys: &str) -> usize {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
    shell.tabs().1
}

/// A repo with `n` commits, tagged `c0` … `c{n-1}`.
fn commits_repo(n: usize) -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for i in 0..n {
        repo.write("notes.txt", format!("note {i}\n").as_bytes());
        repo.commit(&format!("Note {i}"));
        repo.git(&["tag", &format!("c{i}")]);
    }
    repo
}

/// Opens the commit reviews of `c0` … `c{n-1}` in `repo`, in that order.
fn open_commits(shell: &mut Shell, repo: &Path, n: usize) -> Vec<Entity<ReviewTab>> {
    (0..n)
        .map(|i| {
            shell
                .open(commit_req(repo, &format!("refs/tags/c{i}")))
                .unwrap()
        })
        .collect()
}

#[gpui_kit::test]
fn cmd_0_shows_home(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(3);
    let mut shell = start(cx);
    let tabs = open_commits(&mut shell, repo.path(), 3);
    assert_eq!(shell.tabs(), (4, 3));
    assert_eq!(press(&mut shell, "cmd-0"), 0);
    assert!(painted(shell.cx, "home-toolbar").is_some());
    assert!(painted(shell.cx, "review-toolbar").is_none());
    // The open reviews stay open, in order.
    assert_eq!(shell.tabs().0, 4);
    for (i, tab) in tabs.iter().enumerate() {
        assert_eq!(shell.tab(i + 1).review(), Some(tab));
    }
    // From the tree too (any focus inside the window).
    press(&mut shell, "cmd-2");
    click(shell.cx, "tree-row-f:0");
    assert_eq!(press(&mut shell, "cmd-0"), 0);
    assert_eq!(press(&mut shell, "cmd-0"), 0, "on Home already");
}

#[gpui_kit::test]
fn cmd_number_activates_the_nth_open_review(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(3);
    let mut shell = start(cx);
    let tabs = open_commits(&mut shell, repo.path(), 3);
    // ⌘N is the Nth open review (Home is not counted), from Home or from
    // another review.
    for (keys, n) in [("cmd-0", 0), ("cmd-2", 2), ("cmd-1", 1), ("cmd-3", 3)] {
        assert_eq!(press(&mut shell, keys), n, "{keys}");
        if n > 0 {
            assert_eq!(shell.active_review().as_ref(), Some(&tabs[n - 1]));
        }
    }
    // The method the keys call.
    let activated = shell.main.update_in(shell.cx, |m, window, cx| {
        m.activate_review_number(1, window, cx)
    });
    assert!(activated);
    assert_eq!(shell.tabs(), (4, 1));
}

#[gpui_kit::test]
fn cmd_9_activates_the_last_review(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(10);
    let mut shell = start(cx);
    // With no review open, ⌘9 does nothing.
    assert_eq!(press(&mut shell, "cmd-9"), 0);
    assert_eq!(shell.tabs(), (1, 0));
    // Three open: ⌘9 is the third.
    open_commits(&mut shell, repo.path(), 3);
    press(&mut shell, "cmd-0");
    assert_eq!(press(&mut shell, "cmd-9"), 3);
    // Ten open: ⌘8 is the eighth, ⌘9 the last (the tenth), not the ninth.
    let mut tabs = Vec::new();
    for i in 3..10 {
        tabs.push(
            shell
                .open(commit_req(repo.path(), &format!("refs/tags/c{i}")))
                .unwrap(),
        );
    }
    assert_eq!(shell.tabs(), (11, 10));
    assert_eq!(press(&mut shell, "cmd-1"), 1);
    assert_eq!(press(&mut shell, "cmd-8"), 8);
    assert_eq!(press(&mut shell, "cmd-9"), 10);
    assert_eq!(shell.active_review().as_ref(), tabs.last());
}

#[gpui_kit::test]
fn cmd_number_out_of_range_does_nothing(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(2);
    let mut shell = start(cx);
    open_commits(&mut shell, repo.path(), 2);
    assert_eq!(press(&mut shell, "cmd-1"), 1);
    for n in 3..=8 {
        assert_eq!(press(&mut shell, &format!("cmd-{n}")), 1, "cmd-{n}");
        assert_eq!(shell.tabs().0, 3);
    }
    for n in [0, 3, 8] {
        let activated = shell.main.update_in(shell.cx, |m, window, cx| {
            m.activate_review_number(n, window, cx)
        });
        assert!(!activated, "{n}");
        assert_eq!(shell.tabs(), (3, 1));
    }
}

#[gpui_kit::test]
fn toggle_sidebar_action_hides_and_shows(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let pane = |shell: &mut Shell| {
        shell
            .cx
            .update(|window, cx| polygloss_app::keyboard::focused_pane(tab.read(cx), window, cx))
    };
    // ⌃⌘S with the tree's keyboard: hidden, and the diff takes the keyboard.
    click(shell.cx, "tree-row-f:0");
    assert_eq!(pane(&mut shell), Some(polygloss_app::keyboard::Pane::Tree));
    press(&mut shell, "ctrl-cmd-s");
    assert_eq!(sidebar_state(&mut shell), (false, Segment::Files));
    assert!(painted(shell.cx, "sidebar").is_none());
    assert!(painted(shell.cx, "show-sidebar").is_some());
    assert_eq!(
        pane(&mut shell),
        Some(polygloss_app::keyboard::Pane::Viewport)
    );
    // Again: shown, on the segment it had.
    press(&mut shell, "ctrl-cmd-s");
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Files));
    assert!(painted(shell.cx, "file-tree-pane").is_some());
    // On Home too.
    press(&mut shell, "cmd-0");
    press(&mut shell, "ctrl-cmd-s");
    assert!(painted(shell.cx, "sidebar").is_none());
    assert_eq!(bounds(shell.cx, "home-toolbar").left(), px(0.));
    press(&mut shell, "ctrl-cmd-s");
    assert!(painted(shell.cx, "nav").is_some());
}

#[gpui_kit::test]
fn show_files_and_show_reviews_switch_segments(cx: &mut TestAppContext) {
    use polygloss_app::keymap::actions::window::{ShowFiles, ShowReviews};
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let pane = |shell: &mut Shell| {
        shell
            .cx
            .update(|window, cx| polygloss_app::keyboard::focused_pane(tab.read(cx), window, cx))
    };
    click(shell.cx, "tree-row-f:0");
    shell.cx.dispatch_action(ShowReviews);
    draw(shell.cx);
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Reviews));
    assert!(painted(shell.cx, "nav").is_some());
    assert!(painted(shell.cx, "file-tree-pane").is_none());
    assert_eq!(
        pane(&mut shell),
        Some(polygloss_app::keyboard::Pane::Viewport),
        "the hidden tree's keyboard went to the diff"
    );
    shell.cx.dispatch_action(ShowFiles);
    draw(shell.cx);
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Files));
    assert!(painted(shell.cx, "file-tree-pane").is_some());
    // Each also shows a hidden sidebar.
    press(&mut shell, "ctrl-cmd-s");
    shell.cx.dispatch_action(ShowReviews);
    draw(shell.cx);
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Reviews));
    press(&mut shell, "ctrl-cmd-s");
    shell.cx.dispatch_action(ShowFiles);
    draw(shell.cx);
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Files));
}

#[gpui_kit::test]
fn first_open_switches_to_files_refocus_keeps_the_segment(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let chrome = shell.cx.update(|_, cx| chrome::chrome(cx));
    chrome.update(shell.cx, |c, cx| c.set_segment(Segment::Reviews, cx));
    // A first open shows its files.
    let first = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Files));
    assert!(painted(shell.cx, "file-tree-pane").is_some());
    // Opening it again focuses it and keeps Reviews.
    click(shell.cx, "segment-reviews");
    press(&mut shell, "cmd-0");
    let again = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(again, first);
    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(sidebar_state(&mut shell), (true, Segment::Reviews));
    // Another first open switches again, even with the sidebar hidden
    // (it stays hidden).
    press(&mut shell, "ctrl-cmd-s");
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(sidebar_state(&mut shell), (false, Segment::Files));
}

#[gpui_kit::test]
fn files_segment_is_disabled_on_home(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // Home: Files is drawn but takes no click; Reviews shows.
    assert!(painted(shell.cx, "segment-files").is_some());
    click(shell.cx, "segment-files");
    assert!(painted(shell.cx, "nav").is_some());
    assert!(painted(shell.cx, "file-tree-pane").is_none());
    // Its tooltip says why.
    hover(shell.cx, "segment-files");
    shell
        .cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    draw(shell.cx);
    assert!(
        painted(shell.cx, "tooltip: Open a review to see its files").is_some(),
        "the disabled segment explains itself"
    );
    // In a review it switches to the files.
    shell.open(compare_req(repo.path())).unwrap();
    click(shell.cx, "segment-reviews");
    click(shell.cx, "segment-files");
    assert!(painted(shell.cx, "file-tree-pane").is_some());
    hover(shell.cx, "segment-files");
    shell
        .cx
        .executor()
        .advance_clock(std::time::Duration::from_secs(2));
    draw(shell.cx);
    assert!(painted(shell.cx, "tooltip: Files").is_some());
    assert!(painted(shell.cx, "tooltip: Open a review to see its files").is_none());
}

#[gpui_kit::test]
fn menus_name_reviews(cx: &mut TestAppContext) {
    use gpui_kit::OwnedMenuItem;
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    let menus: Vec<(String, Vec<(String, String)>)> = shell.cx.update(|_, cx| {
        cx.get_menus()
            .expect("a menu bar")
            .into_iter()
            .map(|menu| {
                let items = menu
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        OwnedMenuItem::Action { name, action, .. } => {
                            Some((name.to_string(), action.name().to_owned()))
                        }
                        _ => None,
                    })
                    .collect();
                (menu.name.to_string(), items)
            })
            .collect()
    });
    let item = |menu: &str, action: &str| -> Option<String> {
        menus
            .iter()
            .find(|(m, _)| m == menu)
            .and_then(|(_, items)| items.iter().find(|(_, a)| a == action))
            .map(|(name, _)| name.clone())
    };
    assert_eq!(
        item("File", "window::CloseTab").as_deref(),
        Some("Close Review")
    );
    assert_eq!(
        item("Window", "window::NextTab").as_deref(),
        Some("Show Next Review")
    );
    assert_eq!(
        item("Window", "window::PrevTab").as_deref(),
        Some("Show Previous Review")
    );
    assert_eq!(
        item("View", "window::ToggleSidebar").as_deref(),
        Some("Toggle Sidebar")
    );
    assert_eq!(
        item("View", "tab::ToggleThreadsPanel").as_deref(),
        Some("Toggle Threads Panel")
    );
    // No menu item names a tab any more.
    for (menu, items) in &menus {
        for (name, _) in items {
            assert!(!name.contains("Tab"), "{menu} › {name}");
        }
    }
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

    // Find (⌘F) opens the find bar wherever the keyboard is.
    click(&mut shell, "toolbar-find");
    let open = tab.read_with(shell.cx, |t, cx| {
        polygloss_app::find::find_bar(t)
            .expect("a find bar")
            .read(cx)
            .is_open()
    });
    assert!(open, "the toolbar's Find reached the tab");
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
