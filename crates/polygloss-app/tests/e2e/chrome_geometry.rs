//! The chrome as Metal draws it (T7.5, ADR-0031, ADR-0030 Feedback without
//! motion): the sidebar's and the toolbar's edges against the reference
//! (R1, R16–R27, R29, R30), press ink on the custom chrome controls, and
//! changing counts that keep their width.
//!
//! A headless window exposes no debug selectors, so origins are the painted
//! layout: the sidebar's background quad, the divider's 1 pt quad (the main
//! column starts after it), the filter field's, the tree highlight's and
//! the footer's quads, the toolbar's controls and the viewport's
//! `card_bounds`. Glyph and icon ink is `support::ink`, in bands that start
//! inside those boxes. Each reference edge is asserted as
//! `abs(measured − reference) ≤ allowed + 0.5`, both numbers written here
//! from ADR-0031's reference-edges table.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, HeadlessAppContext, Hsla, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, point, px, size,
};
use image::RgbaImage;
use polygloss_app::chrome::{self, Segment};
use polygloss_app::motion::ink::{HOVER, PRESSED};
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::tabs::TabItem;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, threads, viewed, window};
use polygloss_core::git::Source;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};

use crate::support::harness::Test;
use crate::support::ink::Ink;
use crate::support::screenshot::{self, SCALE, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![
    e2e_sidebar_and_toolbar_edges_meet_the_reference,
    e2e_press_ink,
    changing_counts_keep_their_width
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 30;

/// `internal/diff/diff.go` as the reference shows it, then enough lines for
/// 3-digit numbers.
fn diff_go() -> String {
    let mut text = String::from(
        "// Package diff holds the model of a set of changed files and parses the\n\
         // patches git prints into it.\n\
         package diff\n",
    );
    for i in 3..120 {
        text.push_str(&format!("var v{i} = {i}\n"));
    }
    text
}

/// `godiff` as the reference shows it: a worktree named `godiff` whose
/// `init` adds a root `.gitignore` and `internal/diff/diff.go`, with
/// `internal/git/git.go` beside it so `internal` and `diff` are two rows
/// (a lone chain would be compacted into `internal/diff`). Returns the repo
/// and the worktree.
fn godiff_repo() -> (FixtureRepo, PathBuf) {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("README.md", b"# godiff\n");
    repo.commit("readme");
    repo.write(".gitignore", b"/godiff\n");
    repo.write("internal/diff/diff.go", diff_go().as_bytes());
    repo.write(
        "internal/git/git.go",
        b"// Package git runs git.\npackage git\n",
    );
    repo.commit("init");
    let worktree = repo.add_worktree("godiff");
    crate::support::home_above(&worktree);
    (repo, worktree)
}

/// The app at 1280×800 (the Off override) showing `source` of `worktree`
/// once its rows are highlighted, and the store it runs on.
fn open_settled(
    worktree: &Path,
    source: Source,
) -> (HeadlessAppContext, AnyWindowHandle, Core, Entity<ReviewTab>) {
    let core = Core::open_default().expect("open the sandbox store");
    let store = core.clone();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main): (AnyWindowHandle, Entity<MainWindow>) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let req = OpenRequest {
        worktree: worktree.to_path_buf(),
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
        if debug.visible_rows.len() > 3
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
        {
            for _ in 0..3 {
                screenshot::draw(&mut cx, handle);
            }
            return (cx, handle, store, tab);
        }
    }
    panic!("the review tab never settled");
}

/// A painted quad in window points: its bounds and its solid background.
struct Painted {
    bounds: Bounds<f32>,
    background: Option<Hsla>,
    border_top: f32,
}

fn quads(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) -> Vec<Painted> {
    let s = SCALE as f32;
    cx.update_window(handle, |_, window, _| {
        window
            .painted_quads()
            .into_iter()
            .map(|q| Painted {
                bounds: Bounds::new(
                    point(q.bounds.origin.x.0 / s, q.bounds.origin.y.0 / s),
                    size(q.bounds.size.width.0 / s, q.bounds.size.height.0 / s),
                ),
                background: q.background.as_solid(),
                border_top: q.border_widths.top.0 / s,
            })
            .collect()
    })
    .expect("the window is open")
}

/// The one quad of `color` that `pick` keeps.
fn quad_of(
    all: &[Painted],
    color: Hsla,
    what: &str,
    pick: impl Fn(&Bounds<f32>) -> bool,
) -> Bounds<f32> {
    let found: Vec<Bounds<f32>> = all
        .iter()
        .filter(|q| q.background == Some(color) && pick(&q.bounds))
        .map(|q| q.bounds)
        .collect();
    assert_eq!(found.len(), 1, "{what}: {found:?}");
    found[0]
}

/// `abs(measured − reference) ≤ allowed + 0.5`. Prints each measurement
/// for the gate's review (`--no-capture`).
fn assert_edge(name: &str, measured: Option<f32>, reference: f32, allowed: f32) {
    let measured = measured.unwrap_or_else(|| panic!("{name}: no ink in its band"));
    eprintln!("{name}: {measured} pt (the reference's {reference} ± {allowed})");
    assert!(
        (measured - reference).abs() <= allowed + 0.5,
        "{name}: measured {measured}, the reference's {reference} ± {allowed}"
    );
}

/// A band in whole points, rounded inward: `x0..x1` × `y0..y1`.
fn band(x0: f32, x1: f32, y0: f32, y1: f32) -> Bounds<u32> {
    let (x0, y0) = (x0.ceil() as u32, y0.ceil() as u32);
    let (x1, y1) = (x1.floor() as u32, y1.floor() as u32);
    Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0))
}

fn e2e_sidebar_and_toolbar_edges_meet_the_reference() {
    let _sb = Sandbox::isolate();
    let (_repo, worktree) = godiff_repo();
    let (mut cx, handle, _core, tab) =
        open_settled(&worktree, Source::Commit { rev: "HEAD".into() });
    let (sidebar_bg, border, track, highlight) = cx.update(|cx| {
        let t = cx.theme();
        (t.sidebar, t.border, t.tab_bar_segmented, t.list_active)
    });
    let rows = cx.update(|cx| {
        let tree = polygloss_app::tree::file_tree(tab.read(cx))
            .cloned()
            .expect("a file tree");
        tree.read(cx)
            .rows(cx)
            .into_iter()
            .map(|r| (r.label, r.depth))
            .collect::<Vec<_>>()
    });
    let expected: Vec<(String, usize)> = [
        (".gitignore", 0),
        ("internal", 0),
        ("diff", 1),
        ("diff.go", 2),
        ("git", 1),
        ("git.go", 2),
    ]
    .map(|(l, d)| (l.to_owned(), d))
    .to_vec();
    assert_eq!(rows, expected);
    let card = cx.update(|cx| {
        let v = tab.read(cx).viewport.read(cx);
        v.card_bounds(v.display_order()[0])
            .expect("the first card is painted")
    });
    let all = quads(&mut cx, handle);
    let image: RgbaImage = screenshot::capture(&mut cx, handle);
    let ink = Ink::default();

    // R1: the sidebar's background, the divider's 1 pt quad, the main
    // column after it.
    let sidebar = quad_of(&all, sidebar_bg, "the sidebar", |b| {
        b.origin.x == 0.0 && b.origin.y == 0.0
    });
    assert_edge("R1 sidebar", Some(sidebar.size.width), 280.0, 0.0);
    let s = sidebar.origin.x;
    let w = sidebar.size.width;
    let dividers: Vec<Bounds<f32>> = all
        .iter()
        .filter(|q| {
            q.background == Some(border)
                && q.bounds.size.width == 1.0
                && q.bounds.size.height >= WINDOW_HEIGHT / 2.0
        })
        .map(|q| q.bounds)
        .collect();
    assert!(!dividers.is_empty(), "the divider is painted");
    assert!(
        dividers.iter().all(|d| d.origin.x == dividers[0].origin.x),
        "{dividers:?}"
    );
    let divider = dividers[0];
    assert_edge("R1 divider", Some(divider.origin.x - s), 280.0, 0.0);
    assert_edge("R1 divider width", Some(divider.size.width), 1.0, 0.0);
    let main = divider.origin.x + divider.size.width;
    assert_edge("R1 main column", Some(main - s), 281.0, 0.0);

    // R16: the toolbar's first glyph (`godiff`, the repo name on the
    // block's first line), from the main column's edge.
    let repo_name = ink.first_x(&image, band(main + 1.0, main + 80.0, 8.0, 28.0));
    assert_edge(
        "R16 toolbar's first glyph",
        repo_name.map(|x| x - main),
        14.5,
        2.0,
    );

    // R17: the toolbar's last control (Submit review, the rightmost 24 pt
    // quad in the row) against the card's right edge.
    let last = all
        .iter()
        .filter(|q| {
            q.bounds.origin.y < 52.0
                && q.bounds.origin.x > main
                && q.bounds.size.height == 24.0
                && q.background.is_some()
        })
        .map(|q| q.bounds.origin.x + q.bounds.size.width)
        .fold(f32::MIN, f32::max);
    assert_edge(
        "R17 toolbar's last control",
        Some(card.right().as_f32() - last),
        0.0,
        0.0,
    );

    // R29: both segmented tracks.
    let tracks: Vec<Bounds<f32>> = all
        .iter()
        .filter(|q| q.background == Some(track) && q.bounds.origin.y < 52.0)
        .map(|q| q.bounds)
        .collect();
    assert_eq!(tracks.len(), 2, "{tracks:?}");
    for t in &tracks {
        assert_edge("R29 segmented track", Some(t.size.height), 28.0, 0.0);
    }

    // R18: the filter field, from the sidebar's edges and the window's top.
    let field = quad_of(&all, track, "the filter field", |b| {
        b.origin.y >= 52.0 && b.origin.y < 100.0 && b.origin.x < s + w
    });
    assert_edge("R18 field left", Some(field.origin.x - s), 10.0, 0.0);
    assert_edge(
        "R18 field right",
        Some(s + w - (field.origin.x + field.size.width)),
        10.0,
        0.0,
    );
    assert_edge("R18 field top", Some(field.origin.y), 54.0, 0.0);

    // R19: the search icon's ink in the field.
    let (fy0, fy1) = (
        field.origin.y + 2.0,
        field.origin.y + field.size.height - 2.0,
    );
    let search = ink.first_x(
        &image,
        band(field.origin.x + 1.0, field.origin.x + 30.0, fy0, fy1),
    );
    assert_edge("R19 filter icon", search.map(|x| x - s), 19.0, 2.0);

    // The tree: the highlighted row (`.gitignore`, the viewport's top file)
    // is the first; `internal` (depth 0) and `diff` (depth 1) follow.
    let first = quad_of(&all, highlight, "the tree highlight", |b| {
        b.origin.x < s + w && b.origin.y > field.origin.y
    });
    let row = |k: f32| {
        let top = first.origin.y + k * first.size.height;
        (top + 1.0, top + first.size.height - 1.0)
    };
    let (internal, diff) = (row(1.0), row(2.0));
    let chevron = ink.first_x(&image, band(s + 1.0, s + 28.0, internal.0, internal.1));
    assert_edge("R20 depth-0 chevron", chevron.map(|x| x - s), 17.5, 1.0);
    let folder0 = ink.first_x(&image, band(s + 28.0, s + 48.0, internal.0, internal.1));
    assert_edge("R21 depth-0 folder", folder0.map(|x| x - s), 33.5, 1.0);
    let label0 = ink.first_x(&image, band(s + 47.0, s + 120.0, internal.0, internal.1));
    assert_edge("R22 depth-0 label", label0.map(|x| x - s), 52.5, 1.0);
    let label1 = ink.first_x(&image, band(s + 61.0, s + 140.0, diff.0, diff.1));
    assert_edge("R23 depth-1 label", label1.map(|x| x - s), 66.5, 1.0);
    let folder1 = ink.first_x(&image, band(s + 42.0, s + 62.0, diff.0, diff.1));
    assert_edge(
        "R30 tree indent",
        folder1.zip(folder0).map(|(a, b)| a - b),
        14.0,
        0.0,
    );
    let top0 = ink.first_y(&image, band(s + 28.0, s + 48.0, internal.0, internal.1));
    let top1 = ink.first_y(&image, band(s + 42.0, s + 62.0, diff.0, diff.1));
    assert_edge(
        "R24 tree row pitch",
        top1.zip(top0).map(|(a, b)| a - b),
        29.0,
        1.0,
    );

    // R25, R26: the footer (its top border's quad) and `Total:`.
    let footer = all
        .iter()
        .find(|q| {
            q.border_top == 1.0
                && q.bounds.origin.x == s
                && q.bounds.size.width == w
                && q.bounds.origin.y + q.bounds.size.height == WINDOW_HEIGHT
        })
        .map(|q| q.bounds)
        .expect("the footer");
    assert_edge("R26 footer height", Some(footer.size.height), 40.0, 0.0);
    let total = ink.first_x(
        &image,
        band(
            s + 1.0,
            s + 60.0,
            footer.origin.y + 2.0,
            footer.origin.y + footer.size.height - 2.0,
        ),
    );
    assert_edge("R25 footer text", total.map(|x| x - s), 10.5, 1.0);

    // R27: the sidebar toggle's ink, from the sidebar's right edge.
    let toggle = ink.last_x(&image, band(s + w - 36.0, s + w - 1.0, 14.0, 38.0));
    assert_edge("R27 sidebar toggle", toggle.map(|x| s + w - x), 17.0, 0.5);
}

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

fn mouse_down(at: Point<Pixels>) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    })
}

fn mouse_up(at: Point<Pixels>) -> PlatformInput {
    PlatformInput::MouseUp(MouseUpEvent {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
        click_count: 1,
    })
}

/// The pixel at `(x, y)` points of a fresh capture.
fn sample(cx: &mut HeadlessAppContext, window: AnyWindowHandle, at: (u32, u32)) -> [u8; 3] {
    screenshot::draw(cx, window);
    let image = screenshot::capture(cx, window);
    let p = image.get_pixel(at.0 * SCALE, at.1 * SCALE).0;
    [p[0], p[1], p[2]]
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

fn assert_close(actual: [u8; 3], expected: [u8; 3], what: &str) {
    let off = actual
        .iter()
        .zip(expected)
        .any(|(a, e)| a.abs_diff(e) > screenshot::CHANNEL_TOLERANCE);
    assert!(!off, "{what}: {actual:?}, expected {expected:?}");
}

/// Hovers the control under `near`, then checks its ink at a blank point
/// inside it (half its height in from its left, 2 pt below its top, clear
/// of icons and text): the foreground at α 0.06 under the pointer, at
/// α 0.12 while pressed, and at α 0.06 again after the mouse up (on the
/// control when `release_on` says its click changes nothing, else off it,
/// then back).
fn check_press_ink(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    what: &str,
    near: Point<Pixels>,
    release_on: bool,
) {
    let foreground = cx.update(|cx| cx.theme().foreground);
    dispatch(cx, handle, mouse_move(near, None));
    screenshot::draw(cx, handle);
    let hovered: Vec<Bounds<f32>> = quads(cx, handle)
        .into_iter()
        .filter(|q| q.background == Some(foreground.opacity(HOVER)))
        .map(|q| q.bounds)
        .collect();
    assert_eq!(hovered.len(), 1, "{what}: hover ink {hovered:?}");
    let ink = hovered[0];
    let at = (
        (ink.origin.x + ink.size.height / 2.0).round() as u32,
        (ink.origin.y + 2.0).ceil() as u32,
    );
    let p = point(px(at.0 as f32), px(at.1 as f32));
    screenshot::park_pointer(cx, handle);
    let under = sample(cx, handle, at);
    dispatch(cx, handle, mouse_move(p, None));
    assert_close(sample(cx, handle, at), over(foreground, HOVER, under), what);
    dispatch(cx, handle, mouse_down(p));
    assert_close(
        sample(cx, handle, at),
        over(foreground, PRESSED, under),
        &format!("{what}, pressed"),
    );
    if release_on {
        dispatch(cx, handle, mouse_up(p));
    } else {
        let off = point(px(WINDOW_WIDTH / 2.0), px(WINDOW_HEIGHT - 2.0));
        dispatch(cx, handle, mouse_move(off, Some(MouseButton::Left)));
        dispatch(cx, handle, mouse_up(off));
        dispatch(cx, handle, mouse_move(p, None));
    }
    assert_close(
        sample(cx, handle, at),
        over(foreground, HOVER, under),
        &format!("{what}, released"),
    );
    screenshot::park_pointer(cx, handle);
    screenshot::draw(cx, handle);
}

/// `base` then `head` changing `src/lib.rs`, `src/main.rs` and adding
/// `tests/it.rs` (the Tests panel): the accordion shows its headers.
fn categorized_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/lib.rs", b"pub fn lib() {}\n");
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.commit("base");
    repo.write("src/lib.rs", b"pub fn lib() -> u32 {\n    1\n}\n");
    repo.write("src/main.rs", b"fn main() {\n    lib();\n}\n");
    repo.write("tests/it.rs", b"#[test]\nfn it() {}\n");
    repo.commit("head");
    crate::support::home_above(repo.path());
    repo
}

fn e2e_press_ink() {
    let _sb = Sandbox::isolate();
    let repo = categorized_repo();
    let (mut cx, handle, _core, tab) =
        open_settled(repo.path(), Source::Commit { rev: "HEAD".into() });
    let (track_bg, tab_active, list_active) = cx.update(|cx| {
        let t = cx.theme();
        (t.tab_bar_segmented, t.tab_active, t.list_active)
    });
    let all = quads(&mut cx, handle);
    let sidebar_w = 280.0;

    // A segment: the sidebar's selected Files (its click keeps Files).
    let files = quad_of(&all, tab_active, "the selected Files segment", |b| {
        b.origin.y > 0.0 && b.origin.y < 52.0 && b.origin.x < sidebar_w
    });
    let center = |b: Bounds<f32>| point(px(b.center().x), px(b.center().y));
    check_press_ink(&mut cx, handle, "segment", center(files), true);

    // An accordion header: the open Changes panel's (its click keeps it
    // open), the first row under the filter field.
    let field = quad_of(&all, track_bg, "the filter field", |b| {
        b.origin.y >= 52.0 && b.origin.x < sidebar_w
    });
    let header = point(
        px(sidebar_w - 30.0),
        px(field.origin.y + field.size.height + 22.0),
    );
    check_press_ink(&mut cx, handle, "accordion header", header, true);

    // The threads button, shown selected (its click would hide the panel:
    // released off it).
    cx.update(|cx| tab.update(cx, |t, cx| t.set_threads_panel_visible(true, cx)));
    screenshot::draw(&mut cx, handle);
    let all = quads(&mut cx, handle);
    let button = quad_of(&all, list_active, "the threads button", |b| {
        b.origin.y < 52.0 && b.origin.x > sidebar_w
    });
    check_press_ink(&mut cx, handle, "threads button", center(button), false);
    cx.update(|cx| tab.update(cx, |t, cx| t.set_threads_panel_visible(false, cx)));

    // A Reviews row: the active review's (its click keeps the tab).
    cx.update(|cx| chrome::chrome(cx).update(cx, |c, cx| c.set_segment(Segment::Reviews, cx)));
    screenshot::draw(&mut cx, handle);
    let all = quads(&mut cx, handle);
    let row = quad_of(&all, list_active, "the active review's row", |b| {
        b.origin.y > 52.0 && b.origin.x < sidebar_w
    });
    check_press_ink(&mut cx, handle, "Reviews row", center(row), true);
}

/// 88 changed files, `f00.rs` to `f87.rs`.
fn many_files_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for i in 0..88 {
        repo.write(&format!("f{i:02}.rs"), b"fn f() {}\n");
    }
    repo.commit("base");
    for i in 0..88 {
        repo.write(&format!("f{i:02}.rs"), b"fn f() {}\nfn g() {}\n");
    }
    repo.commit("head");
    crate::support::home_above(repo.path());
    repo
}

/// ADR-0031 (no layout shift): `N/M` and the threads count keep their
/// width as their digits change (11 → 88, 11 → 44), so the toolbar's right cluster
/// stays put: the threads button (shown selected, so it paints its box)
/// keeps its bounds. Measured with real shaping: the app binary's test
/// platform draws every glyph one width.
fn changing_counts_keep_their_width() {
    let _sb = Sandbox::isolate();
    let repo = many_files_repo();
    let (mut cx, handle, core, tab) =
        open_settled(repo.path(), Source::Commit { rev: "HEAD".into() });
    let list_active = cx.update(|cx| cx.theme().list_active);
    cx.update(|cx| tab.update(cx, |t, cx| t.set_threads_panel_visible(true, cx)));
    let button = |cx: &mut HeadlessAppContext| {
        screenshot::draw(cx, handle);
        screenshot::draw(cx, handle);
        let all = quads(cx, handle);
        quad_of(&all, list_active, "the threads button", |b| {
            b.origin.y < 52.0 && b.origin.x > 280.0
        })
    };
    let view = |cx: &mut HeadlessAppContext, files: std::ops::Range<u32>| {
        let files: Vec<u32> = files.collect();
        cx.update(|cx| tab.update(cx, |t, cx| viewed::set_viewed(t, &files, true, cx)));
    };
    let label = |cx: &mut HeadlessAppContext| cx.update(|cx| viewed::progress_label(tab.read(cx)));

    view(&mut cx, 0..11);
    let eleven = button(&mut cx);
    assert_eq!(label(&mut cx), "11/88");
    view(&mut cx, 11..88);
    let all_viewed = button(&mut cx);
    assert_eq!(label(&mut cx), "88/88");
    assert_eq!(eleven, all_viewed, "N/M kept its width");

    // The threads count: 11 open threads, then 44 (an agent opens at most
    // 50 in a review).
    let opened = cx.update(|cx| tab.read(cx).opened.clone());
    let blobs = BlobReader::open(&opened.repo).expect("open the object store");
    let add = |cx: &mut HeadlessAppContext, n: usize| {
        for _ in 0..n {
            core.create_thread(
                &NewThread {
                    review_id: opened.review_id.clone(),
                    diff_id: opened.diff_id.clone(),
                    subject: Subject::Line {
                        path: "f00.rs".into(),
                        side: Side::New,
                        start_line: 1,
                        line: 1,
                    },
                    kind: ThreadKind::Comment,
                    body_md: "Count me.".into(),
                    // An agent's: open, not a draft (a draft would also
                    // count on Submit review).
                    author: Author {
                        kind: AuthorKind::Agent,
                        name: "claude-code".into(),
                        session_id: None,
                    },
                },
                &blobs,
            )
            .expect("create a thread");
        }
        cx.update(|cx| tab.update(cx, threads::reload));
    };
    add(&mut cx, 11);
    let few = button(&mut cx);
    assert_eq!(cx.update(|cx| threads::open_counts(tab.read(cx), cx)).0, 11);
    add(&mut cx, 33);
    let many = button(&mut cx);
    assert_eq!(cx.update(|cx| threads::open_counts(tab.read(cx), cx)).0, 44);
    assert_eq!(few, many, "the threads count kept its width");
}
