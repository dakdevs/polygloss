//! The card reveal in the full app (T7.8, ADR-0030 M3), drawn by Metal: its
//! filmstrips (a card collapsing by its chevron, in place and pinned) and
//! the window's settling around it, through the review tab's registration
//! with `motion::settle`: a mouse down freezes the reveal where it is
//! painted, so the click lands on what was pressed; a key down and a scroll
//! settle it.
//!
//! The harness names these tests `motion_viewport::<test>` (the module, as
//! the plan's filter selects them; the `tests!` macro names a test after
//! its function alone).
//!
//! Fixture: a commit adding `a.rs` (60 lines: a card taller than the
//! viewport, so its collapse takes `exit(200)` = 150 ms), `b.rs` (10),
//! `c.rs` (30) and `d.rs` (10), in a 1280 × 800 window under the header
//! card. Durations are written here from ADR-0030's tokens.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, Entity, HeadlessAppContext, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent,
    point, px, size,
};
use polygloss_app::motion::{self, MotionPolicy, tokens};
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::tabs::TabItem;
use polygloss_app::tree::file_tree;
use polygloss_app::{startup, window};
use polygloss_core::git::Source;
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;
use polygloss_viewport::{ControlAction, DiffViewport, ViewportDebug};

use crate::support::filmstrip::{assert_filmstrip, filmstrip};
use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &[
    Test::new(
        "motion_viewport::e2e_motion_card_collapse",
        e2e_motion_card_collapse,
    ),
    Test::new(
        "motion_viewport::e2e_motion_card_collapse_pinned",
        e2e_motion_card_collapse_pinned,
    ),
    Test::new(
        "motion_viewport::a_click_pressed_mid_reveal_hits_the_frozen_frame",
        a_click_pressed_mid_reveal_hits_the_frozen_frame,
    ),
    Test::new(
        "motion_viewport::a_press_on_a_row_mid_reveal_moves_the_cursor_only",
        a_press_on_a_row_mid_reveal_moves_the_cursor_only,
    ),
    Test::new(
        "motion_viewport::a_key_down_with_the_sidebar_focused_settles_the_reveal",
        a_key_down_with_the_sidebar_focused_settles_the_reveal,
    ),
    Test::new(
        "motion_viewport::a_scroll_settles_the_reveal",
        a_scroll_settles_the_reveal,
    ),
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 40;
/// The first step after a commit (ADR-0030 `FIRST_STEP`).
const FIRST_STEP: Duration = Duration::from_micros(16_667);

fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

fn lines(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("let {prefix}_{i} = {i};\n"))
        .collect()
}

/// A README, then a commit adding `a.rs` … `d.rs`.
fn cards_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("README.md", b"# cards\n");
    repo.commit("readme");
    for (name, n) in [("a.rs", 60), ("b.rs", 10), ("c.rs", 30), ("d.rs", 10)] {
        repo.write(name, lines(&name[..1], n).as_bytes());
    }
    repo.commit("add four files");
    repo
}

/// The app and its review tab. `cx` is the last field: fields drop in
/// order, and the app checks for leaked entity handles when it drops.
struct App {
    window: AnyWindowHandle,
    viewport: Entity<DiffViewport>,
    tab: Entity<ReviewTab>,
    cx: HeadlessAppContext,
}

/// The app at 1280 × 800 showing the commit once its rows are highlighted,
/// under the harness's Off policy.
fn open_settled(repo: &FixtureRepo) -> App {
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (window, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, window);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit { rev: "HEAD".into() },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let _task = cx
        .update_window(window, |_, w, cx| open_review(req, w, cx))
        .expect("the window is open");
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, window);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        let Some(tab) = tab else { continue };
        let viewport = cx.update(|cx| tab.read(cx).viewport.clone());
        let d = cx.update(|cx| viewport.read(cx).debug());
        if d.visible_rows.len() > 10
            && !d.visible_rows.iter().any(|r| r == "Loading…")
            && d.styled_rows > 0
        {
            for _ in 0..3 {
                screenshot::draw(&mut cx, window);
            }
            return App {
                cx,
                window,
                viewport,
                tab,
            };
        }
    }
    panic!("the review tab never settled");
}

impl App {
    fn debug(&mut self) -> ViewportDebug {
        let viewport = self.viewport.clone();
        self.cx.update(|cx| viewport.read(cx).debug())
    }

    fn running(&mut self) -> bool {
        let viewport = self.viewport.clone();
        self.cx.update(|cx| viewport.read(cx).motion_running())
    }

    /// The file in display slot `slot`.
    fn file(&mut self, slot: usize) -> u32 {
        let viewport = self.viewport.clone();
        self.cx.update(|cx| viewport.read(cx).display_order()[slot])
    }

    fn policy(&mut self, policy: MotionPolicy) {
        self.cx.update(|cx| motion::set_override(Some(policy), cx));
    }

    /// The viewport's origin in the window (its debug frame is relative to
    /// it), from a painted card.
    fn origin(&mut self) -> (f32, f32) {
        let d = self.debug();
        let card = d.cards[0];
        let viewport = self.viewport.clone();
        let bounds = self
            .cx
            .update(|cx| viewport.read(cx).card_bounds(card.file_idx))
            .expect("a painted card");
        (
            bounds.origin.x.as_f32() - card.bounds.0,
            bounds.origin.y.as_f32() - card.bounds.1,
        )
    }

    /// The centre of the control doing `action`, in window points.
    fn control(&mut self, action: ControlAction) -> Point<Pixels> {
        let d = self.debug();
        let (x, y, w, h) = d
            .controls
            .iter()
            .find(|c| c.action == action)
            .unwrap_or_else(|| panic!("no control {action:?}"))
            .bounds;
        let o = self.origin();
        point(px(o.0 + x + w / 2.0), px(o.1 + y + h / 2.0))
    }

    fn dispatch(&mut self, input: PlatformInput) {
        self.cx
            .update_window(self.window, |_, w, cx| {
                w.dispatch_event(input, cx);
            })
            .expect("the window is open");
    }

    fn press(&mut self, at: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }));
    }

    fn release(&mut self, at: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        }));
        self.cx.run_until_parked();
    }

    /// A click on file `f`'s chevron, as painted.
    fn click_chevron(&mut self, f: u32) {
        let at = self.control(ControlAction::Collapse(f));
        self.press(at);
        self.release(at);
    }

    /// Delivers the frames motion requested and draws one; how many were
    /// requested.
    fn frame(&mut self) -> usize {
        let requested = self
            .cx
            .update_window(self.window, |_, w, cx| w.simulate_next_frame(cx))
            .expect("the window is open");
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, w, cx| w.render_frame(cx))
            .expect("the window is open");
        requested
    }

    fn advance(&mut self, by: Duration) -> usize {
        self.cx.advance_clock(by);
        self.frame()
    }

    /// From the commit, the frame `since` later by the stepping protocol.
    fn step_to(&mut self, since: Duration) {
        let first = since.min(FIRST_STEP);
        self.advance(first);
        if since > first {
            self.advance(since - first);
        }
    }

    /// File `f`'s header as painted.
    fn header_y(&mut self, f: u32) -> f32 {
        let d = self.debug();
        d.headers
            .iter()
            .find(|h| h.file_idx == f)
            .unwrap_or_else(|| panic!("no header for file {f}: {:?}", d.headers))
            .y
    }

    /// Collapses (or expands) file `f` at once, under Off.
    fn snap(&mut self, f: u32, collapsed: bool) {
        self.policy(MotionPolicy::Off);
        let viewport = self.viewport.clone();
        self.cx
            .update(|cx| viewport.update(cx, |v, cx| v.set_collapsed(f, collapsed, cx)));
        screenshot::draw(&mut self.cx, self.window);
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

/// `a.rs`'s card below its header, its padding included: over 500 pt, so
/// its reveal takes BASE (200 ms) and its collapse `exit(200)` = 150 ms.
fn assert_tall(app: &mut App, f: u32) {
    let viewport = app.viewport.clone();
    let below = app.cx.update(|cx| {
        let doc = viewport.read(cx).document();
        (doc.card_bottom(f) - doc.body_top(f)) as f32
    });
    assert!(below >= 500.0, "a.rs is {below} pt below its header");
}

fn e2e_motion_card_collapse() {
    let _sb = Sandbox::isolate();
    let repo = cards_repo();
    let mut app = open_settled(&repo);
    let a = app.file(0);
    assert_tall(&mut app, a);
    let window = app.window;
    let at = app.control(ControlAction::Collapse(a));
    let click = move |cx: &mut HeadlessAppContext| click_at(cx, window, at);
    let full = filmstrip(&mut app.cx, window, MotionPolicy::Full, ms(150.0), click);
    app.snap(a, false);
    screenshot::park_pointer(&mut app.cx, window);
    screenshot::draw(&mut app.cx, window);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        click,
    );
    assert_filmstrip("card-collapse", full, reduced);
}

fn e2e_motion_card_collapse_pinned() {
    let _sb = Sandbox::isolate();
    let repo = cards_repo();
    let mut app = open_settled(&repo);
    let a = app.file(0);
    assert_tall(&mut app, a);
    let viewport = app.viewport.clone();
    // a.rs's header pinned at the top with 300 pt of its body scrolled past.
    let pin = move |cx: &mut HeadlessAppContext| {
        cx.update(|cx| {
            viewport.update(cx, |v, cx| {
                let past = v.document().header_top(a) + 300.0 - v.document().scroll_top();
                v.scroll_by(past as f32, cx)
            })
        });
    };
    pin(&mut app.cx);
    screenshot::draw(&mut app.cx, app.window);
    assert!(
        app.debug().headers[0].sticky,
        "the header is pinned: {:?}",
        app.debug().headers
    );
    let window = app.window;
    let at = app.control(ControlAction::Collapse(a));
    let click = move |cx: &mut HeadlessAppContext| click_at(cx, window, at);
    let full = filmstrip(&mut app.cx, window, MotionPolicy::Full, ms(150.0), click);
    app.snap(a, false);
    pin(&mut app.cx);
    screenshot::park_pointer(&mut app.cx, window);
    screenshot::draw(&mut app.cx, window);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        click,
    );
    assert_filmstrip("card-collapse-pinned", full, reduced);
}

/// `a.rs` collapsed (under Off), so `b.rs`, `c.rs` and `d.rs` are on
/// screen; then `b.rs` collapsed too, and Full for what follows.
fn with_b_closed(app: &mut App) -> [u32; 4] {
    let files = [app.file(0), app.file(1), app.file(2), app.file(3)];
    app.snap(files[0], true);
    app.snap(files[1], true);
    app.policy(MotionPolicy::Full);
    files
}

fn a_click_pressed_mid_reveal_hits_the_frozen_frame() {
    let _sb = Sandbox::isolate();
    let repo = cards_repo();
    let mut app = open_settled(&repo);
    let [_, b, c, d] = with_b_closed(&mut app);
    // b.rs (208 pt) opens: reveal(208) = 170.8 ms.
    app.click_chevron(b);
    assert!(app.running());
    app.step_to(ms(60.0));
    let viewport = app.viewport.clone();
    // How far c.rs's card reaches below its header while it is open.
    let c_body = app.cx.update(|cx| {
        let doc = viewport.read(cx).document();
        (doc.card_bottom(c) - doc.body_top(c)) as f32
    });
    let c_header = app.header_y(c);
    let pressed = app.control(ControlAction::Collapse(c));
    app.press(pressed);
    let frozen = app.debug();
    assert!(
        frozen.reveal.expect("revealing").frozen,
        "the mouse down froze it"
    );
    // The frame the 60 ms one asked for, then three frozen ones.
    let counts: Vec<usize> = (0..4).map(|_| app.advance(FIRST_STEP)).collect();
    assert_eq!(counts[1..], [0, 0, 0], "no frame while frozen: {counts:?}");
    assert_eq!(app.header_y(c), c_header, "the press held the frame");
    assert_eq!(app.control(ControlAction::Collapse(c)), pressed);
    // The release there toggles c.rs: b.rs ends opened, c.rs closes.
    app.release(pressed);
    app.frame();
    let (b_open, c_closed) = app.cx.update(|cx| {
        let doc = viewport.read(cx).document();
        (!doc.is_collapsed(b), doc.is_collapsed(c))
    });
    assert!(b_open && c_closed, "the click landed on c.rs's chevron");
    let reveal = app.debug().reveal.expect("c.rs's reveal runs on");
    assert_eq!(reveal.file_idx, c);
    assert!(!reveal.frozen);
    // c.rs (608 pt) closes over exit(200) = 150 ms: d.rs rises from below
    // the viewport, where its card was (c.rs's body above it).
    app.step_to(ms(75.0));
    let d_mid = app.header_y(d);
    app.advance(ms(100.0));
    let d_new = app.header_y(d);
    assert!(!app.running());
    let d_old = d_new + c_body;
    assert!(
        d_mid < d_old && d_mid > d_new,
        "d.rs between its old and new y at ½: {d_old} > {d_mid} > {d_new}"
    );
}

fn a_press_on_a_row_mid_reveal_moves_the_cursor_only() {
    let _sb = Sandbox::isolate();
    let repo = cards_repo();
    let mut app = open_settled(&repo);
    let [_, b, ..] = with_b_closed(&mut app);
    app.click_chevron(b);
    app.step_to(ms(60.0));
    // A row of b.rs under the curtain.
    let d = app.debug();
    let curtain = d.reveal.expect("revealing").curtain;
    let (row, (y, h)) = d
        .visible_rows
        .iter()
        .zip(d.row_bounds.iter().copied())
        .find(|(t, (y, h))| t.contains("b_2 ") && y + h < curtain)
        .map(|(t, b)| (t.clone(), b))
        .expect("b.rs's third row is painted");
    let o = app.origin();
    let at = point(px(o.0 + 300.0), px(o.1 + y + h / 2.0));
    app.press(at);
    let viewport = app.viewport.clone();
    let cursor = app.cx.update(|cx| viewport.read(cx).cursor());
    let cursor = cursor.expect("the press put the cursor down");
    assert_eq!((cursor.file_idx, cursor.line), (b, 2), "on {row}");
    let held = app.debug();
    assert!(held.reveal.expect("still revealing").frozen);
    for _ in 0..3 {
        app.advance(FIRST_STEP);
    }
    assert_eq!(app.debug().reveal, held.reveal, "held until the mouse up");
    app.release(at);
    app.frame();
    assert!(!app.running(), "the mouse up settled it");
}

fn a_key_down_with_the_sidebar_focused_settles_the_reveal() {
    let _sb = Sandbox::isolate();
    let repo = cards_repo();
    let mut app = open_settled(&repo);
    let [_, b, ..] = with_b_closed(&mut app);
    let tab = app.tab.clone();
    let tree = app
        .cx
        .update(|cx| file_tree(tab.read(cx)).cloned())
        .expect("a file tree");
    let focus_tree = |app: &mut App| {
        let tree = tree.clone();
        app.cx
            .update_window(app.window, |_, w, cx| {
                tree.update(cx, |t, cx| t.focus(w, cx))
            })
            .expect("the window is open");
    };
    app.click_chevron(b);
    // The tree has the keyboard (a click in the diff may have taken it).
    focus_tree(&mut app);
    app.step_to(ms(60.0));
    assert!(app.running());
    let viewport = app.viewport.clone();
    let cursor = app.cx.update(|cx| viewport.read(cx).cursor());
    app.cx
        .update_window(app.window, |_, w, cx| {
            w.dispatch_keystroke(Keystroke::parse("j").expect("a keystroke"), cx);
        })
        .expect("the window is open");
    assert!(!app.running(), "the key down settled the reveal");
    let in_tree = app
        .cx
        .update_window(app.window, |_, w, cx| tree.read(cx).contains_focus(w, cx))
        .expect("the window is open");
    assert!(in_tree, "the tree kept the keyboard");
    assert_eq!(
        app.cx.update(|cx| viewport.read(cx).cursor()),
        cursor,
        "`j` went to the tree, not the diff"
    );
}

fn a_scroll_settles_the_reveal() {
    let _sb = Sandbox::isolate();
    let repo = cards_repo();
    let mut app = open_settled(&repo);
    let [_, b, ..] = with_b_closed(&mut app);
    app.click_chevron(b);
    app.step_to(ms(60.0));
    assert!(app.running());
    let o = app.origin();
    app.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
        position: point(px(o.0 + 300.0), px(o.1 + 300.0)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
        modifiers: Modifiers::default(),
        ..Default::default()
    }));
    assert!(!app.running(), "the scroll settled the reveal");
}
