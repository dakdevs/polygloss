//! The app shell (T3.1): the main window with its tab bar, a review tab's
//! toolbar, banner strip and panes around the viewport, as the app draws it
//! (gpui-kit initialized, its icons bundled).

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
    assert_screenshot(&image);
}
