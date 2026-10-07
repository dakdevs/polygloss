//! The chrome's motions in the full app (T7.9, ADR-0030 M10–M14), drawn by
//! Metal: each one's filmstrip, Full over Reduced (`support::filmstrip`).
//!
//! - `segmented`: a pointer picks Reviews in the sidebar's `[Files |
//!   Reviews]` (M11).
//! - `tree-chevron`: a pointer folds the tree's first folder (M10).
//! - `banner`: a live-changes notice enters (M12); `banner-exit`: it leaves
//!   an empty strip, as its inert copy (the commit frame shows the copy
//!   exactly where the notice was, pixel for pixel).
//! - `open-flow-step`: `⏎` picks the recent repo in ⌘O (M13).
//! - `home-row`: a Home row archived by pointer (M14).
//!
//! The harness names these tests `motion_chrome::<test>`. A headless window
//! has no debug selectors, so the two pointer clicks land at points
//! computed here from the layout tokens, and each test checks that its
//! click did what it should. Windows are small, so the strips stay small.

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, Entity, HeadlessAppContext, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, point, px, size,
};
use polygloss_app::chrome::{self, Segment};
use polygloss_app::home::HomeView;
use polygloss_app::motion::{self, Initiator, MotionPolicy, tokens};
use polygloss_app::open_flow::{self, FlowStep};
use polygloss_app::review_tab::{BannerKind, ReviewTab, open_review};
use polygloss_app::space::{edge, gap, height, layout, pad};
use polygloss_app::tabs::TabItem;
use polygloss_app::tree::file_tree;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::{Since, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::filmstrip::{assert_filmstrip, filmstrip, frame_of};
use crate::support::harness::Test;
use crate::support::screenshot;
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &[
    Test::new("motion_chrome::e2e_motion_segmented", e2e_motion_segmented),
    Test::new(
        "motion_chrome::e2e_motion_tree_chevron",
        e2e_motion_tree_chevron,
    ),
    Test::new("motion_chrome::e2e_motion_banner", e2e_motion_banner),
    Test::new(
        "motion_chrome::e2e_motion_banner_exit",
        e2e_motion_banner_exit,
    ),
    Test::new(
        "motion_chrome::e2e_motion_open_flow_step",
        e2e_motion_open_flow_step,
    ),
    Test::new("motion_chrome::e2e_motion_home_row", e2e_motion_home_row),
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 40;
/// The smallest main window (`layout::WINDOW_MIN`).
const SMALL: (f32, f32) = layout::WINDOW_MIN;

/// `lib/util.rs` and `src/{app,ui}.rs` added: the tree's rows are `lib`,
/// `util.rs`, `src`, `app.rs`, `ui.rs`, in one Changes panel.
fn folders_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("notes.txt", b"notes\n");
    repo.commit("notes");
    for (path, body) in [
        ("lib/util.rs", "pub fn util() {}\n"),
        ("src/app.rs", "pub fn app() {}\n"),
        ("src/ui.rs", "pub fn ui() {}\n"),
    ] {
        repo.write(path, body.as_bytes());
    }
    repo.commit("add three files");
    repo
}

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
    /// Opens the commit at `repo`'s HEAD and draws until its rows are
    /// highlighted.
    fn open_commit(&mut self, repo: &FixtureRepo) -> Entity<ReviewTab> {
        let req = OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Commit { rev: "HEAD".into() },
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

fn segment(cx: &mut HeadlessAppContext) -> Segment {
    cx.update(|cx| chrome::chrome(cx).read(cx).segment())
}

fn e2e_motion_segmented() {
    let _sb = Sandbox::isolate();
    let repo = folders_repo();
    let mut app = open_app(SMALL.0, SMALL.1);
    app.open_commit(&repo);
    assert_eq!(segment(&mut app.cx), Segment::Files);
    // Reviews, the second segment: the sidebar's top row ends `CANVAS` in,
    // after the toggle and a `CONTROLS` gap; the track's rim and the
    // segment's half width before that.
    let reviews = point(
        px(layout::SIDEBAR_WIDTH
            - edge::CANVAS
            - height::SM
            - gap::CONTROLS
            - pad::RIM
            - height::MD / 2.0),
        px(height::TOP / 2.0),
    );
    let window = app.window;
    let pick = move |cx: &mut HeadlessAppContext| {
        click_at(cx, window, reviews);
        assert_eq!(segment(cx), Segment::Reviews, "the click picked Reviews");
    };
    let full = filmstrip(&mut app.cx, window, MotionPolicy::Full, tokens::QUICK, pick);
    app.reset(|cx| {
        cx.update(|cx| chrome::chrome(cx).update(cx, |c, cx| c.set_segment(Segment::Files, cx)))
    });
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        pick,
    );
    assert_filmstrip("segmented", full, reduced);
}

fn e2e_motion_tree_chevron() {
    let _sb = Sandbox::isolate();
    let repo = folders_repo();
    let mut app = open_app(SMALL.0, SMALL.1);
    let tab = app.open_commit(&repo);
    let tree = app
        .cx
        .update(|cx| file_tree(tab.read(cx)).cloned().expect("a file tree"));
    let expanded = move |cx: &mut HeadlessAppContext| {
        let tree = tree.clone();
        cx.update(|cx| tree.read(cx).expanded_dirs())
    };
    assert_eq!(expanded(&mut app.cx), ["lib", "src"]);
    // The first row, `lib`: under the top row, the filter field (`RIM`
    // above it, `CONTROLS` below), half a row down.
    let lib = point(
        px(layout::SIDEBAR_WIDTH / 2.0),
        px(height::TOP + pad::RIM + height::MD + gap::CONTROLS + height::MD / 2.0),
    );
    let window = app.window;
    let fold = {
        let expanded = expanded.clone();
        move |cx: &mut HeadlessAppContext| {
            click_at(cx, window, lib);
            assert_eq!(expanded(cx), ["src"], "the click folded lib");
        }
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::MICRO,
        fold.clone(),
    );
    let tree = app
        .cx
        .update(|cx| file_tree(tab.read(cx)).cloned().expect("a file tree"));
    app.reset(|cx| {
        cx.update(|cx| {
            tree.update(cx, |t, cx| {
                t.set_expanded_dirs(["lib".to_string(), "src".to_string()], cx)
            })
        })
    });
    assert_eq!(expanded(&mut app.cx), ["lib", "src"]);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        fold,
    );
    assert_filmstrip("tree-chevron", full, reduced);
}

/// The live-changes notice shown (`true`) or cleared in `tab`'s strip.
fn notice(cx: &mut HeadlessAppContext, tab: &Entity<ReviewTab>, shown: bool) {
    let banners = cx.update(|cx| tab.read(cx).banners.clone());
    cx.update(|cx| {
        banners.update(cx, |b, cx| {
            if shown {
                b.set(
                    BannerKind::LiveChanges,
                    "2 files changed".into(),
                    Box::new(polygloss_app::keymap::actions::tab::Refresh),
                    cx,
                )
            } else {
                b.clear(BannerKind::LiveChanges, cx)
            }
        })
    });
}

fn e2e_motion_banner() {
    let _sb = Sandbox::isolate();
    let repo = folders_repo();
    let mut app = open_app(SMALL.0, SMALL.1);
    let tab = app.open_commit(&repo);
    let window = app.window;
    let show = {
        let tab = tab.clone();
        move |cx: &mut HeadlessAppContext| notice(cx, &tab, true)
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::QUICK,
        show.clone(),
    );
    app.reset(|cx| notice(cx, &tab, false));
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        show,
    );
    assert_filmstrip("banner", full, reduced);
}

fn e2e_motion_banner_exit() {
    let _sb = Sandbox::isolate();
    let repo = folders_repo();
    let mut app = open_app(SMALL.0, SMALL.1);
    let tab = app.open_commit(&repo);
    app.reset(|cx| notice(cx, &tab, true));
    let shown = app
        .cx
        .capture_screenshot(app.window)
        .expect("render the frame with Metal");
    let window = app.window;
    let clear = {
        let tab = tab.clone();
        move |cx: &mut HeadlessAppContext| notice(cx, &tab, false)
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::MICRO,
        clear.clone(),
    );
    // The copy's commit frame is the notice as it was shown.
    let comparison = screenshot::compare(&frame_of(&full, 0), &shown)
        .expect("one window's frames have one size");
    assert!(
        comparison.within_budget(),
        "the leaving copy differs from the live notice: {comparison:?}"
    );
    app.reset(|cx| notice(cx, &tab, true));
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        clear,
    );
    assert_filmstrip("banner-exit", full, reduced);
}

fn e2e_motion_open_flow_step() {
    let _sb = Sandbox::isolate();
    let repo = folders_repo();
    // A recent repo: `⏎` in the repo list picks it.
    Core::open_default()
        .unwrap()
        .open(&OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Live { since: Since::Head },
            label: None,
            pin: None,
            actor: Actor::human(),
        })
        .unwrap();
    let mut app = open_app(800.0, 600.0);
    let window = app.window;
    let open = move |cx: &mut HeadlessAppContext| {
        cx.update_window(window, |_, w, cx| {
            w.dispatch_action(
                Box::new(polygloss_app::keymap::actions::window::OpenFlow),
                cx,
            )
        })
        .expect("the window is open");
        for _ in 0..4 {
            screenshot::draw(cx, window);
        }
    };
    open(&mut app.cx);
    let flow = app
        .cx
        .update(|cx| open_flow::current(cx))
        .expect("⌘O opened the flow");
    let enter = {
        let flow = flow.clone();
        move |cx: &mut HeadlessAppContext| {
            cx.update_window(window, |_, w, cx| {
                w.dispatch_keystroke(Keystroke::parse("enter").expect("a keystroke"), cx);
            })
            .expect("the window is open");
            for _ in 0..MAX_FRAMES {
                cx.run_until_parked();
                if cx.update(|cx| flow.read(cx).step()) == FlowStep::Source {
                    return;
                }
            }
            panic!("⏎ never showed the source step");
        }
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::QUICK,
        enter.clone(),
    );
    app.reset(|cx| {
        cx.update_window(window, |_, w, cx| {
            flow.update(cx, |f, cx| f.back(Initiator::Programmatic, w, cx))
        })
        .expect("the window is open");
    });
    assert_eq!(app.cx.update(|cx| flow.read(cx).step()), FlowStep::Repo);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        enter,
    );
    assert_filmstrip("open-flow-step", full, reduced);
}

/// Four commit reviews of `repo`, `c0` the most recently active.
fn home_reviews(repo: &FixtureRepo) -> Vec<String> {
    let core = Core::open_default().unwrap();
    (0..4)
        .map(|i| {
            let opened = core
                .open(&OpenRequest {
                    worktree: repo.path().to_path_buf(),
                    source: Source::Commit {
                        rev: format!("refs/tags/c{i}"),
                    },
                    label: None,
                    pin: None,
                    actor: Actor::human(),
                })
                .unwrap();
            let at = screenshot::CLOCK_MS - (i as i64 + 1) * 3_600_000;
            core.store
                .write(|tx| {
                    tx.execute(
                        &format!(
                            "UPDATE reviews SET updated_at = {at} WHERE id = '{}'",
                            opened.review_id
                        ),
                        (),
                    )?;
                    Ok(())
                })
                .unwrap();
            opened.review_id
        })
        .collect()
}

fn e2e_motion_home_row() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    for i in 0..4 {
        repo.write("notes.txt", format!("note {i}\n").as_bytes());
        repo.commit(&format!("Note {i}"));
        repo.git(&["tag", &format!("c{i}")]);
    }
    let ids = home_reviews(&repo);
    // Wide enough for the rows' titles beside their fixed columns.
    let mut app = open_app(1100.0, SMALL.1);
    let main = app.main.clone();
    let home: Entity<HomeView> = app.cx.update(|cx| match main.read(cx).tabs().get(0) {
        Some(TabItem::Home(home)) => home.clone(),
        _ => panic!("tab 0 is Home"),
    });
    app.cx.update(|cx| {
        home.update(cx, |h, cx| {
            h.set_clock(|| screenshot::CLOCK_MS, cx);
            h.refresh(cx);
        })
    });
    for _ in 0..MAX_FRAMES {
        app.draw();
        if app.cx.update(|cx| home.read(cx).rows().len()) == 4 {
            break;
        }
    }
    assert_eq!(app.cx.update(|cx| home.read(cx).rows().len()), 4);
    let window = app.window;
    let archive = {
        let (home, id) = (home.clone(), ids[1].clone());
        move |cx: &mut HeadlessAppContext| {
            cx.update_window(window, |_, w, cx| {
                home.update(cx, |h, cx| h.archive(id.clone(), Initiator::Pointer, w, cx))
            })
            .expect("the window is open");
            cx.run_until_parked();
        }
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::exit(tokens::BASE),
        archive.clone(),
    );
    // Back: the review opened again is un-archived.
    app.reset(|cx| {
        Core::open_default()
            .unwrap()
            .open(&OpenRequest {
                worktree: repo.path().to_path_buf(),
                source: Source::Commit {
                    rev: "refs/tags/c1".into(),
                },
                label: None,
                pin: None,
                actor: Actor::human(),
            })
            .unwrap();
        let at = screenshot::CLOCK_MS - 2 * 3_600_000;
        Core::open_default()
            .unwrap()
            .store
            .write(|tx| {
                tx.execute(
                    &format!(
                        "UPDATE reviews SET updated_at = {at} WHERE id = '{}'",
                        ids[1]
                    ),
                    (),
                )?;
                Ok(())
            })
            .unwrap();
        cx.update(|cx| home.update(cx, |h, cx| h.refresh(cx)));
    });
    assert_eq!(app.cx.update(|cx| home.read(cx).rows().len()), 4);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        archive,
    );
    assert_filmstrip("home-row", full, reduced);
}
