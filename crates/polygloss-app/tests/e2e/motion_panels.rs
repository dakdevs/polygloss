//! The panels' motions in the full app (T7.10, ADR-0030 M1–M2), drawn by
//! Metal: each one's filmstrip, Full over Reduced (`support::filmstrip`).
//!
//! - `sidebar`: a pointer hides the sidebar with its toggle (M1): the frame
//!   closes to the window's leading edge with the content riding its
//!   divider, the toolbar's items follow it to their hidden places, and the
//!   show-sidebar button is uncovered where it rests, after the traffic
//!   lights' room (a headless window has no traffic lights).
//! - `threads-panel`: the review's threads panel opens from the window's
//!   trailing edge (M2) as the toolbar's button opens it; the toolbar does
//!   not move.
//!
//! The harness names these tests `motion_panels::<test>`. A headless window
//! has no debug selectors, so the sidebar's toggle is clicked at a point
//! computed here from the layout tokens, and the test checks the click hid
//! the sidebar.

use std::sync::Arc;

use gpui_kit::{
    AnyWindowHandle, Entity, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, point, px, size,
};
use polygloss_app::chrome;
use polygloss_app::motion::{self, Initiator, MotionPolicy, tokens};
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::space::{edge, height, layout};
use polygloss_app::tabs::TabItem;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;

use crate::support::filmstrip::{assert_filmstrip, filmstrip};
use crate::support::harness::Test;
use crate::support::screenshot;
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

pub const TESTS: &[Test] = &[
    Test::new("motion_panels::e2e_motion_sidebar", e2e_motion_sidebar),
    Test::new(
        "motion_panels::e2e_motion_threads_panel",
        e2e_motion_threads_panel,
    ),
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 40;

/// The app and the main window. `cx` is the last field: fields drop in
/// order, and the app checks for leaked entity handles when it drops.
struct App {
    window: AnyWindowHandle,
    main: Entity<MainWindow>,
    cx: HeadlessAppContext,
}

fn open_app(width: f32, height: f32) -> App {
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (window, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(width), px(height)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, window);
    screenshot::draw(&mut cx, window);
    App { window, main, cx }
}

impl App {
    /// Opens `repo`'s `base..head` and draws until its rows are
    /// highlighted.
    fn open_compare(&mut self, repo: &FixtureRepo) -> Entity<ReviewTab> {
        let req = OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Compare {
                base: "refs/tags/base".into(),
                head: "refs/tags/head".into(),
                mode: CompareMode::Direct,
            },
            label: None,
            pin: None,
            actor: Actor::human(),
        };
        let _task = self
            .cx
            .update_window(self.window, |_, w, cx| open_review(req, w, cx))
            .expect("the window is open");
        let main = self.main.clone();
        for _ in 0..MAX_FRAMES {
            self.draw();
            let tab = self.cx.update(|cx| {
                main.read(cx)
                    .tabs()
                    .get(1)
                    .and_then(TabItem::review)
                    .cloned()
            });
            let Some(tab) = tab else { continue };
            let viewport = self.cx.update(|cx| tab.read(cx).viewport.clone());
            let d = self.cx.update(|cx| viewport.read(cx).debug());
            if !d.visible_rows.is_empty()
                && !d.visible_rows.iter().any(|r| r == "Loading…")
                && d.styled_rows > 0
            {
                for _ in 0..3 {
                    self.draw();
                }
                return tab;
            }
        }
        panic!("the review tab never settled");
    }

    fn draw(&mut self) {
        screenshot::draw(&mut self.cx, self.window);
    }

    /// Under Off, `change` then a settled frame (between a strip's rows).
    fn reset(&mut self, change: impl FnOnce(&mut HeadlessAppContext)) {
        self.cx
            .update(|cx| motion::set_override(Some(MotionPolicy::Off), cx));
        change(&mut self.cx);
        screenshot::park_pointer(&mut self.cx, self.window);
        for _ in 0..3 {
            self.draw();
        }
    }
}

/// A pointer click at `at` in `window`: a move there, a press, a release.
fn click_at(cx: &mut HeadlessAppContext, window: AnyWindowHandle, at: Point<Pixels>) {
    for input in [
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }),
        PlatformInput::MouseDown(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    ] {
        cx.update_window(window, |_, w, cx| {
            w.dispatch_event(input, cx);
        })
        .expect("the window is open");
    }
    cx.run_until_parked();
}

fn sidebar_visible(cx: &mut HeadlessAppContext) -> bool {
    cx.update(|cx| chrome::chrome(cx).read(cx).sidebar_visible())
}

fn e2e_motion_sidebar() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut app = open_app(layout::WINDOW_MIN.0, layout::WINDOW_MIN.1);
    app.open_compare(&repo);
    assert!(sidebar_visible(&mut app.cx));
    // The sidebar's toggle: the top row's last control, `CANVAS` from the
    // sidebar's trailing edge, centred in the row.
    let toggle = point(
        px(layout::SIDEBAR_WIDTH - edge::CANVAS - height::SM / 2.0),
        px(height::TOP / 2.0),
    );
    let window = app.window;
    let hide = move |cx: &mut HeadlessAppContext| {
        click_at(cx, window, toggle);
        assert!(!sidebar_visible(cx), "the click hid the sidebar");
        // The pointer leaves the toolbar, so no hover ink is in the strip.
        screenshot::park_pointer(cx, window);
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::exit(tokens::PANEL),
        hide,
    );
    app.reset(|cx| {
        cx.update(|cx| chrome::chrome(cx).update(cx, |c, cx| c.set_sidebar_visible(true, cx)))
    });
    assert!(sidebar_visible(&mut app.cx));
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        hide,
    );
    assert_filmstrip("sidebar", full, reduced);
}

/// Wide enough that the threads panel keeps 340 pt and the sidebar its
/// 280 pt (a narrower window gives the sidebar's room to the panel).
const THREADS_WINDOW: (f32, f32) = (1100.0, 480.0);

fn e2e_motion_threads_panel() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut app = open_app(THREADS_WINDOW.0, THREADS_WINDOW.1);
    let tab = app.open_compare(&repo);
    let window = app.window;
    let shown = {
        let tab = tab.clone();
        move |cx: &mut HeadlessAppContext| cx.update(|cx| tab.read(cx).threads_panel_visible())
    };
    // As the toolbar's threads button does (its click is the app tests').
    let open = {
        let (tab, shown) = (tab.clone(), shown.clone());
        move |cx: &mut HeadlessAppContext| {
            cx.update_window(window, |_, w, cx| {
                tab.update(cx, |t, cx| {
                    t.set_threads_shown_by(true, Initiator::Pointer, w, cx)
                })
            })
            .expect("the window is open");
            assert!(shown(cx), "the panel shows");
        }
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::PANEL,
        open.clone(),
    );
    app.reset(|cx| cx.update(|cx| tab.update(cx, |t, cx| t.set_threads_panel_visible(false, cx))));
    assert!(!shown(&mut app.cx));
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        open,
    );
    assert_filmstrip("threads-panel", full, reduced);
}
