//! Find across all files (T3.15, design §11.14): the find bar over the diff
//! of the code-change fixture, with a query, its count, the toggles and the
//! result list, the second match gone to (selected in the list, the line
//! cursor on it).

use std::sync::Arc;

use gpui_kit::{px, size};
use polygloss_app::find::find_bar;
use polygloss_app::review_tab::open_review;
use polygloss_app::tabs::TabItem;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{Sandbox, code_change_repo};

pub const TESTS: &[Test] = &crate::tests![e2e_find_bar_results];

/// Frames drawn at most while waiting for loads, the search and highlights.
const MAX_FRAMES: usize = 30;

fn e2e_find_bar_results() {
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
    let mut tab = None;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        if tab.is_some() {
            break;
        }
    }
    let tab = tab.expect("the review tab opened");
    let bar = cx
        .update(|cx| find_bar(tab.read(cx)).cloned())
        .expect("a find bar");
    cx.update_window(handle, |_, window, cx| {
        bar.update(cx, |b, cx| {
            b.open(window, cx);
            b.set_query("entries", window, cx);
        })
    })
    .expect("the window is open");
    let mut searched = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        if cx.update(|cx| !bar.read(cx).is_searching()) {
            searched = true;
            break;
        }
    }
    assert!(searched, "the search never finished");
    cx.update(|cx| bar.update(cx, |b, cx| (b.next(cx), b.next(cx))));

    // Wait for the diff to paint around the match, highlighted.
    let mut settled = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let debug = cx.update(|cx| tab.read(cx).viewport.read(cx).debug());
        if debug.visible_rows.len() > 10
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
        {
            settled = true;
            break;
        }
    }
    assert!(settled, "the review tab never settled");
    screenshot::draw(&mut cx, handle);
    let (label, count) = cx.update(|cx| {
        let b = bar.read(cx);
        (b.status_label(), b.matches().len())
    });
    assert!(count > 4, "{count} matches");
    assert_eq!(label, format!("2 of {count}"));
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
