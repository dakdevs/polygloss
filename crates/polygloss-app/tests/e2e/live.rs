//! Screenshot of T3.11 (design §10, §11.4, §11.7): a live review whose
//! working tree changed after it was shown. The toolbar has the base picker
//! ("Base: merge base") and Snapshot; the banner strip says "1 file changed"
//! with its Refresh (R) button, and the diff below has not moved.

use std::sync::Arc;

use gpui_kit::{px, size};
use polygloss_app::live;
use polygloss_app::review_tab::{BannerKind, open_review};
use polygloss_app::tabs::TabItem;
use polygloss_app::{startup, window};
use polygloss_core::git::{Since, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_live_banner];

/// Frames drawn at most while waiting for loads, highlights and the
/// recompute.
const MAX_FRAMES: usize = 30;

fn e2e_live_banner() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("src/config.rs", CONFIG_RS_HEAD.as_bytes());

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
        source: Source::Live {
            since: Since::MergeBase,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    let mut settled = None;
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
            settled = Some(tab);
            break;
        }
    }
    let tab = settled.expect("the review tab settled");

    // An agent adds a file; the recompute (what the watcher runs after a
    // batch) finds it.
    repo.write("src/main.rs", b"mod config;\n\nfn main() {}\n");
    cx.update(|cx| tab.update(cx, live::recompute_now));
    let mut shown = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let banners = cx.update(|cx| tab.read(cx).banners.read(cx).banners());
        if banners
            .iter()
            .any(|(k, t)| *k == BannerKind::LiveChanges && t == "1 file changed")
        {
            shown = true;
            break;
        }
    }
    assert!(shown, "the live banner showed");
    screenshot::draw(&mut cx, handle);
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
