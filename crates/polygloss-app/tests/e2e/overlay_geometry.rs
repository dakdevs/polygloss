//! Home and overlay geometry as Metal draws it (T7.6, ADR-0031, ADR-0030
//! Feedback without motion): press ink on a Home row, the file finder over
//! a review (`e2e_overlay_finder`: the picker frame, `MD` rows with their
//! status letters, names and folders, the first one selected) and the base
//! picker over a live review (`e2e_overlay_base_picker`: the same frame, its
//! heading as the list's section header, `ROW2` rows with the check slot,
//! subject, author and date, and the SHA pills; the dates are read at a
//! pinned clock, so they are the same on every run).

use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyWindowHandle, Entity, HeadlessAppContext, Hsla, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, point, px, size,
};
use image::RgbaImage;
use polygloss_app::home::HomeView;
use polygloss_app::keymap::actions::window as window_actions;
use polygloss_app::live::base_picker;
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::tabs::TabItem;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::harness::Test;
use crate::support::screenshot::{self, SCALE, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{
    CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox, code_change_repo, open_compare,
};

pub const TESTS: &[Test] = &crate::tests![
    e2e_press_ink_home,
    e2e_overlay_finder,
    e2e_overlay_base_picker
];

/// Frames drawn at most while Home loads.
const MAX_FRAMES: usize = 20;

fn dispatch(cx: &mut HeadlessAppContext, window: AnyWindowHandle, input: PlatformInput) {
    cx.update_window(window, |_, window, cx| window.dispatch_event(input, cx))
        .expect("window is open");
}

fn mouse_move(at: Point<Pixels>, pressed: Option<MouseButton>) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: at,
        pressed_button: pressed,
        modifiers: Modifiers::default(),
    })
}

/// The pixel at `(x, y)` points of a capture.
fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = image.get_pixel(x * SCALE, y * SCALE).0;
    [p[0], p[1], p[2]]
}

/// A fresh capture's pixel at `at`.
fn sample(cx: &mut HeadlessAppContext, window: AnyWindowHandle, at: (u32, u32)) -> [u8; 3] {
    screenshot::draw(cx, window);
    pixel(&screenshot::capture(cx, window), at.0, at.1)
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| a.abs_diff(b) <= screenshot::CHANNEL_TOLERANCE)
}

fn assert_close(actual: [u8; 3], expected: [u8; 3], what: &str) {
    assert!(
        close(actual, expected),
        "{what}: {actual:?}, expected {expected:?}"
    );
}

fn rgb8(color: Hsla) -> [u8; 3] {
    let c = color.to_rgb();
    [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8)
}

/// `color` at `alpha` over `under`, as 8-bit sRGB.
fn over(color: Hsla, alpha: f32, under: [u8; 3]) -> [u8; 3] {
    let c = rgb8(color);
    std::array::from_fn(|i| {
        (f32::from(c[i]) * alpha + f32::from(under[i]) * (1.0 - alpha)).round() as u8
    })
}

/// A blank point inside the first Home card: the first pixel column, left
/// to right, holding a vertical run of the card's background at least 55 pt
/// tall, at the run's middle. A card is 60 tall with its border outside the
/// run (58); the toolbar row above the page (51 without its divider) and
/// sidebar rows are shorter.
fn blank_point_in_a_card(image: &RgbaImage, card: [u8; 3]) -> (u32, u32) {
    let (w, h) = (WINDOW_WIDTH as u32, WINDOW_HEIGHT as u32);
    for x in (0..w).step_by(4) {
        let mut run = 0;
        for y in 0..h {
            if close(pixel(image, x, y), card) {
                run += 1;
            } else {
                if run >= 55 {
                    return (x, y - run / 2);
                }
                run = 0;
            }
        }
    }
    panic!("no Home card found");
}

fn e2e_press_ink_home() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    open_compare(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main): (AnyWindowHandle, Entity<MainWindow>) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let home: Entity<HomeView> = cx.update(|cx| match main.read(cx).tabs().get(0) {
        Some(TabItem::Home(home)) => home.clone(),
        _ => panic!("Home is the first tab"),
    });
    cx.update(|cx| home.update(cx, |h, cx| h.refresh(cx)));
    let mut loaded = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        if cx.update(|cx| home.read(cx).is_loaded() && !home.read(cx).rows().is_empty()) {
            loaded = true;
            break;
        }
    }
    assert!(loaded, "Home never listed the review");
    screenshot::draw(&mut cx, handle);

    let (foreground, card) = cx.update(|cx| {
        (
            cx.theme().foreground,
            polygloss_app::theme::viewport_theme(cx).card_background,
        )
    });
    let image = screenshot::capture(&mut cx, handle);
    let at = blank_point_in_a_card(&image, rgb8(card));
    let row = pixel(&image, at.0, at.1);
    let p = point(px(at.0 as f32), px(at.1 as f32));

    dispatch(&mut cx, handle, mouse_move(p, None));
    assert_close(
        sample(&mut cx, handle, at),
        over(foreground, 0.06, row),
        "hovered",
    );
    dispatch(
        &mut cx,
        handle,
        PlatformInput::MouseDown(MouseDownEvent {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
    );
    let pressed = sample(&mut cx, handle, at);
    assert_close(pressed, over(foreground, 0.12, row), "pressed");
    // Released off the row (no click, so the review does not open), then
    // back on it: hovered again, not pressed.
    let off = point(px(at.0 as f32), px(WINDOW_HEIGHT - 2.0));
    dispatch(&mut cx, handle, mouse_move(off, Some(MouseButton::Left)));
    dispatch(
        &mut cx,
        handle,
        PlatformInput::MouseUp(MouseUpEvent {
            position: off,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    );
    dispatch(&mut cx, handle, mouse_move(p, None));
    assert_close(
        sample(&mut cx, handle, at),
        over(foreground, 0.06, row),
        "released, hovered again",
    );
    screenshot::park_pointer(&mut cx, handle);
    assert_close(sample(&mut cx, handle, at), row, "the pointer left");
    let tabs = cx.update(|cx| main.read(cx).tabs().len());
    assert_eq!(tabs, 1, "no click: the review did not open");
}

fn e2e_overlay_finder() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut cx, handle, _tab) = settled_review(
        &repo,
        Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
    );
    cx.update_window(handle, |_, window, cx| {
        window.dispatch_action(Box::new(window_actions::FileFinder), cx)
    })
    .expect("the window is open");
    for _ in 0..4 {
        screenshot::draw(&mut cx, handle);
    }
    let open = cx.update(|cx| polygloss_app::tree::finder::current(cx).is_some());
    assert!(open, "the finder is open");
    assert_screenshot(&screenshot::capture(&mut cx, handle));
}

/// The app over `repo` (`HOME` above it; the Off override, so gpui-kit's
/// dialogs settle on their first frame) with `source` open in a settled
/// review tab.
fn settled_review(
    repo: &FixtureRepo,
    source: Source,
) -> (HeadlessAppContext, AnyWindowHandle, Entity<ReviewTab>) {
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    cx.update(|cx| {
        use polygloss_app::motion::{MotionPolicy, set_override};
        set_override(Some(MotionPolicy::Off), cx)
    });
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let (handle, main): (AnyWindowHandle, Entity<MainWindow>) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source,
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    cx.update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open")
        .detach();
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        let Some(tab) = tab else { continue };
        let debug = cx.update(|cx| tab.read(cx).viewport.read(cx).debug());
        if debug.visible_rows.len() > 10
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
        {
            return (cx, handle, tab);
        }
    }
    panic!("the review tab never settled");
}

/// The base picker's "now": 2026-01-01 01:00 UTC, an hour after
/// `FixtureRepo`'s first commit (its commits are a minute apart from
/// 2026-01-01 00:00 UTC).
const BASE_PICKER_NOW_MS: i64 = (1_767_225_600 + 60 * 60) * 1000;

fn e2e_overlay_base_picker() {
    let _sb = Sandbox::isolate();
    // Four commits on main, three on `feature` (checked out) and an
    // uncommitted edit: more bases than the frame shows.
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("README.md", b"# app\n");
    repo.commit("Initial commit");
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.commit("Add the config parser");
    repo.write("app.conf", b"greeting = Hello\n");
    repo.commit("Ship a sample app.conf");
    repo.write("README.md", b"# app\n\nReads `app.conf` at startup.\n");
    repo.commit("Document app.conf in the README");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("src/config.rs", CONFIG_RS_HEAD.as_bytes());
    repo.commit("Keep config entries sorted by key");
    repo.write("app.conf", b"greeting = Hello\nname = World\n");
    repo.commit("Add a name to the sample config");
    repo.write("CHANGELOG.md", b"- Sorted config entries\n");
    repo.commit("Start a changelog");
    repo.write("src/main.rs", b"mod config;\n\nfn main() {}\n");

    let (mut cx, handle, tab) = settled_review(
        &repo,
        Source::Live {
            since: Since::MergeBase,
        },
    );
    let picker = cx
        .update_window(handle, |_, window, cx| {
            tab.update(cx, |t, cx| base_picker::open(t, window, cx))
        })
        .expect("the window is open")
        .expect("a live review has a base picker");
    let mut loaded = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        if cx.update(|cx| !picker.read(cx).delegate().loading()) {
            loaded = true;
            break;
        }
    }
    assert!(loaded, "the base picker never read the log");
    let bases = cx.update(|cx| picker.read(cx).delegate().matches().len());
    assert_eq!(bases, 2 + 7, "the moving bases, then every commit");
    cx.update(|cx| {
        picker.update(cx, |p, cx| {
            p.delegate_mut().set_clock(BASE_PICKER_NOW_MS, 0);
            cx.notify();
        })
    });
    for _ in 0..4 {
        screenshot::draw(&mut cx, handle);
    }
    assert_screenshot(&screenshot::capture(&mut cx, handle));
}
