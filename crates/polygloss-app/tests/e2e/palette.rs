//! The command palette and the cheat sheet (T3.2) over a review tab, as the
//! app draws them (gpui-kit initialized, its icons bundled).

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::keymap::actions::window as window_actions;
use polygloss_app::review_tab::open_review;
use polygloss_app::tabs::TabItem;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

pub const TESTS: &[Test] = &crate::tests![e2e_palette_command_palette, e2e_palette_cheat_sheet];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 20;

/// The app with `code_change_repo`'s `base..head` open and settled.
fn review_window(
    cx: &mut HeadlessAppContext,
) -> (AnyWindowHandle, Entity<MainWindow>, FixtureRepo) {
    // The dialogs' entrance animation settles on its first frame, so the
    // capture never catches them mid-slide (a machine under load drew the
    // four frames of `open` before the animation ended).
    cx.update(|cx| cx.set_reduce_motion(true));
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let (handle, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(cx, handle);
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
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    let mut settled = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(cx, handle);
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
    screenshot::draw(cx, handle);
    (handle, main, repo)
}

/// Dispatches `action` from the focused element and lets the dialog settle.
fn open(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, action: Box<dyn gpui_kit::Action>) {
    cx.update_window(handle, |_, window, cx| window.dispatch_action(action, cx))
        .expect("the window is open");
    for _ in 0..4 {
        screenshot::draw(cx, handle);
    }
}

fn e2e_palette_command_palette() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, _main, _repo) = review_window(&mut cx);
    open(&mut cx, handle, Box::new(window_actions::CommandPalette));
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

fn e2e_palette_cheat_sheet() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, _main, _repo) = review_window(&mut cx);
    open(&mut cx, handle, Box::new(window_actions::CheatSheet));
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
