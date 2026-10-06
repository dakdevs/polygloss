//! The app shell (T3.1, T6.3): the main window as the app draws it
//! (gpui-kit initialized, the app's icons bundled): the sidebar (top row,
//! file tree) beside a review's main column (toolbar row, banner strip,
//! viewport and threads panel).

use std::sync::Arc;

use gpui_kit::{px, size};
use polygloss_app::review_tab::open_review;
use polygloss_app::tabs::TabItem;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{Sandbox, code_change_repo};

pub const TESTS: &[Test] = &crate::tests![e2e_shell_review_tab];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 20;

fn e2e_shell_review_tab() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            // Full names: on a case-insensitive disk `head` alone is HEAD.
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    let mut settled = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        if let Some(tab) = tab {
            let viewport = cx.update(|cx| tab.read(cx).viewport.clone());
            let debug = cx.update(|cx| viewport.read(cx).debug());
            if debug.visible_rows.len() > 10
                && !debug.visible_rows.iter().any(|r| r == "Loading…")
                && debug.styled_rows > 0
            {
                settled = true;
                break;
            }
        }
    }
    assert!(settled, "the review tab never settled");
    // One more frame for the background pass (header counts).
    screenshot::draw(&mut cx, handle);
    screenshot::draw(&mut cx, handle);
    let (tabs, active) = cx.update(|cx| {
        let m = main.read(cx);
        (m.tabs().len(), m.tabs().active())
    });
    assert_eq!((tabs, active), (2, 1));
    let image = screenshot::capture(&mut cx, handle);
    assert_sidebar_divider_is_one_point(&image);
    assert_screenshot(&image);
}

/// The sidebar is 280 pt of its color and the divider to the main column one
/// point of the border color, as in the reference (a 2 px line at 2×):
/// across the top rows, at 2×, x 0–559 are the sidebar, 560–561 the divider,
/// 562 the toolbar (Polygloss Light's palette, by hand).
fn assert_sidebar_divider_is_one_point(image: &image::RgbaImage) {
    let y = 26 * screenshot::SCALE;
    let at = |x: u32| image.get_pixel(x, y).0;
    let (sidebar, border, toolbar) = (
        [0xeb, 0xeb, 0xea, 0xff],
        [0xe7, 0xe7, 0xe7, 0xff],
        [0xfc, 0xfc, 0xfb, 0xff],
    );
    let row: Vec<_> = (540..580).map(at).collect();
    assert_eq!(at(10), sidebar, "the sidebar's top row");
    assert_eq!(at(559), sidebar, "the sidebar is 280 pt: {row:x?}");
    assert_eq!((at(560), at(561)), (border, border), "{row:x?}");
    assert_eq!(at(562), toolbar, "a one-point divider: {row:x?}");
}
