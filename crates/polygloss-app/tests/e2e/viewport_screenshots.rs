//! Viewport screenshots (plan T2.8): a fixture repo opened through `Core`,
//! read through `CoreDiffProvider` and painted by a `DiffViewport` that fills
//! a 1280×800 @2x window with a pinned Pierre theme, compared with
//! `tests/baselines/<test-name>.png` by `support::screenshot`.
//!
//! Each test first checks what the frame shows (from `ViewportDebug`), so a
//! baseline recorded with `UPDATE_BASELINE=1` cannot silently accept a wrong
//! frame, and captures only once every visible row is loaded and carries its
//! syntax tokens.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::AppContext as _;
use image::RgbaImage;
use polygloss_app::CoreDiffProvider;
use polygloss_core::review::OpenedDiff;
use polygloss_diff::rows::Layout;
use polygloss_highlight::Appearance;
use polygloss_viewport::{
    DiffProvider, DiffViewport, FrameStats, LayoutMode, ViewportDebug, ViewportEvent,
    ViewportOptions, ViewportTheme,
};

use crate::support::harness::Test;
use crate::support::screenshot::{self, assert_screenshot};
use crate::support::{Sandbox, code_change_repo, open_compare, special_files_repo};

pub const TESTS: &[Test] = &crate::tests![
    e2e_viewport_split_pierre_light,
    e2e_viewport_unified_pierre_dark,
    e2e_viewport_special_files,
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 20;

struct Shot {
    image: RgbaImage,
    debug: ViewportDebug,
    stats: FrameStats,
}

/// Renders `opened` in a viewport with `layout` and Pierre `appearance` and
/// captures the first frame whose visible rows are all loaded and
/// highlighted.
fn render(opened: &OpenedDiff, layout: LayoutMode, appearance: Appearance) -> Shot {
    let provider: Arc<dyn DiffProvider> =
        Arc::new(CoreDiffProvider::open(opened).expect("open the provider"));
    let opts = ViewportOptions {
        layout,
        theme: Arc::new(ViewportTheme::pierre(appearance)),
        ..ViewportOptions::default()
    };
    let mut cx = screenshot::headless_app();
    let window = screenshot::open_window(&mut cx, |window, cx| {
        cx.new(|cx| DiffViewport::new(provider, opts, window, cx))
    });
    let viewport = window.root(&mut cx).expect("window has a root view");
    let frames: Rc<RefCell<Vec<FrameStats>>> = Rc::default();
    let sink = frames.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&viewport, move |_, event: &ViewportEvent, _| {
            if let ViewportEvent::FrameStats(stats) = event {
                sink.borrow_mut().push(*stats);
            }
        })
    });

    let settled = |frames: &[FrameStats]| {
        frames
            .last()
            .is_some_and(|s| s.visible_rows > 0 && s.loading_rows == 0 && s.unhighlighted_rows == 0)
    };
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, *window);
        if settled(&frames.borrow()) {
            break;
        }
    }
    let stats = *frames.borrow().last().expect("a frame was drawn");
    assert!(
        settled(&frames.borrow()),
        "the frame never settled within {MAX_FRAMES} frames: {stats:?}"
    );
    let image = screenshot::capture(&mut cx, *window);
    let debug = viewport.read_with(&cx, |v, _| v.debug());
    Shot {
        image,
        debug,
        stats,
    }
}

/// A split row as `ViewportDebug::visible_rows` prints it.
fn split(left: Option<(u32, char, &str)>, right: Option<(u32, char, &str)>) -> String {
    let cell = |c: Option<(u32, char, &str)>| match c {
        Some((n, m, t)) => format!("{n:>5} {m} {t}"),
        None => format!("{:>5} {} {}", "", ' ', ""),
    };
    format!("{} │ {}", cell(left), cell(right))
}

/// A unified row as `ViewportDebug::visible_rows` prints it.
fn unified(old: Option<u32>, new: Option<u32>, marker: char, text: &str) -> String {
    let n = |v: Option<u32>| v.map_or(String::new(), |v| v.to_string());
    format!("{:>5} {:>5} {} {}", n(old), n(new), marker, text)
}

fn assert_rows(debug: &ViewportDebug, expected: &[String]) {
    for row in expected {
        assert!(
            debug.visible_rows.contains(row),
            "{row:?} not in {:#?}",
            debug.visible_rows
        );
    }
}

fn e2e_viewport_split_pierre_light() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let shot = render(
        &open_compare(repo.path()),
        LayoutMode::Split,
        Appearance::Light,
    );
    assert_eq!(shot.debug.layout, Layout::Split);
    assert_eq!(shot.debug.visible_rows[0], "== src/config.rs");
    assert_rows(
        &shot.debug,
        &[
            split(
                Some((1, '-', "use std::collections::HashMap;")),
                Some((1, '+', "use std::collections::BTreeMap;")),
            ),
            split(
                Some((10, '-', "        let mut entries = HashMap::new();")),
                Some((10, '+', "        let mut entries = BTreeMap::new();")),
            ),
            split(Some((29, ' ', "    }")), Some((29, ' ', "    }"))),
            split(
                None,
                Some((31, '+', "    pub fn is_empty(&self) -> bool {")),
            ),
        ],
    );
    // A gap row hides the unchanged middle of `parse`, and the next file
    // starts on the first screen.
    let greet = shot
        .debug
        .visible_rows
        .iter()
        .position(|r| r == "== src/greet.ts");
    assert!(greet.is_some(), "{:#?}", shot.debug.visible_rows);
    assert!(shot.debug.styled_rows > 0);
    assert_eq!(shot.stats.unhighlighted_rows, 0);
    assert_screenshot(&shot.image);
}

fn e2e_viewport_unified_pierre_dark() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let shot = render(
        &open_compare(repo.path()),
        LayoutMode::Unified,
        Appearance::Dark,
    );
    assert_eq!(shot.debug.layout, Layout::Unified);
    assert_eq!(shot.debug.visible_rows[0], "== src/config.rs");
    assert_rows(
        &shot.debug,
        &[
            unified(Some(1), None, '-', "use std::collections::HashMap;"),
            unified(None, Some(1), '+', "use std::collections::BTreeMap;"),
            unified(Some(2), Some(2), ' ', ""),
        ],
    );
    assert!(shot.debug.styled_rows > 0);
    assert_screenshot(&shot.image);
}

fn e2e_viewport_special_files() {
    let _sb = Sandbox::isolate();
    let repo = special_files_repo();
    let shot = render(
        &open_compare(repo.path()),
        LayoutMode::Split,
        Appearance::Light,
    );
    let rows = &shot.debug.visible_rows;
    for header in [
        "== Cargo.lock",
        "== assets/logo.png",
        "== current",
        "== docs/old-guide.md → docs/guide.md",
        "== legacy.txt",
        "== scripts/build.sh",
        "== vendor/lib",
    ] {
        assert!(
            rows.iter().any(|r| r.starts_with(header)),
            "no header {header:?} in {rows:#?}"
        );
    }
    assert!(
        rows.iter().any(|r| r.starts_with("Binary file")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("a1b2c3d → d4e5f60")),
        "{rows:#?}"
    );
    // The symlink's target and the deleted file's lines are real rows.
    assert_rows(
        &shot.debug,
        &[
            split(Some((1, '-', "v1")), Some((1, '+', "v2"))),
            split(Some((1, '-', "This file is no longer used.")), None),
        ],
    );
    assert_screenshot(&shot.image);
}
