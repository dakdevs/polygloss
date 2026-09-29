//! Screenshot of T3.12 (design §11.4, OQ-9): a compare review reviewed at
//! iteration 1, then moved on to iteration 2, shown as **Changes since last
//! review**. The toolbar's picker is highlighted and reads "Changes since
//! last review"; its menu is open, with the checked toggle and both
//! iterations; the banner line names both heads and says comments go on
//! new lines only; the diff below has only what changed since the review
//! (`src/config.rs` pre-sized, `src/main.rs` added), not `src/greet.ts`.

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{px, size};
use polygloss_app::review_tab::open_review;
use polygloss_app::tabs::TabItem;
use polygloss_app::{iterations, live, startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest, Verdict};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![
    e2e_iterations_changes_since,
    e2e_iterations_changes_since_menu
];

/// Frames drawn at most while waiting for loads, highlights and switches.
const MAX_FRAMES: usize = 30;

const GREET_BASE: &str =
    "export function greet(name: string): string {\n  return \"Hello, \" + name + \"!\";\n}\n";
const GREET_HEAD: &str = "export function greet(name: string, greeting = \"Hello\"): string {\n  return `${greeting}, ${name}!`;\n}\n";
const MAIN_RS: &str = "mod config;\n\nfn main() {\n    let text = std::fs::read_to_string(\"app.conf\").unwrap_or_default();\n    let config = config::Config::parse(&text);\n    println!(\"{} settings\", config.len());\n}\n";

/// Changes since last review, as the tab shows it.
fn e2e_iterations_changes_since() {
    let _sb = Sandbox::isolate();
    let mut shown = changes_since();
    screenshot::draw(&mut shown.cx, shown.handle);
    let image = screenshot::capture(&mut shown.cx, shown.handle);
    assert_screenshot(&image);
}

/// The same with the picker's menu open.
fn e2e_iterations_changes_since_menu() {
    let _sb = Sandbox::isolate();
    let mut shown = changes_since();
    let handle = shown.handle;
    shown
        .cx
        .update_window(handle, |_, window, cx| {
            window.click("iteration-picker", cx);
        })
        .expect("the window is open");
    for _ in 0..4 {
        screenshot::draw(&mut shown.cx, handle);
    }
    screenshot::park_pointer(&mut shown.cx, handle);
    screenshot::draw(&mut shown.cx, handle);
    let image = screenshot::capture(&mut shown.cx, handle);
    assert_screenshot(&image);
}

/// The app showing the changes since the review. Fields drop in order: the
/// tab before the app, the repo last.
struct Shown {
    _tab: gpui_kit::Entity<polygloss_app::review_tab::ReviewTab>,
    handle: gpui_kit::AnyWindowHandle,
    cx: gpui_kit::HeadlessAppContext,
    _repo: FixtureRepo,
}

/// Opens the review, submits it, moves the branch on, refreshes and turns
/// on Changes since last review (module docs).
fn changes_since() -> Shown {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.write("src/greet.ts", GREET_BASE.as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("src/config.rs", CONFIG_RS_HEAD.as_bytes());
    repo.write("src/greet.ts", GREET_HEAD.as_bytes());
    repo.commit("feature: parse into a BTreeMap");

    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core.clone(), cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::ThreeDot,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    let mut opened = None;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        opened = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        if opened.is_some() {
            break;
        }
    }
    let tab = opened.expect("the review tab opened");

    // Reviewed at iteration 1; then the branch moves on.
    let review_id = cx.update(|cx| tab.read(cx).review_id.clone());
    core.submit_review(&review_id, Verdict::RequestChanges, "Pre-size it", None)
        .expect("submit");
    repo.write(
        "src/config.rs",
        CONFIG_RS_HEAD
            .replace(
                "let mut entries = BTreeMap::new();",
                "let mut entries: BTreeMap<String, String> = BTreeMap::new();",
            )
            .as_bytes(),
    );
    repo.write("src/main.rs", MAIN_RS.as_bytes());
    repo.commit("feature: pre-size, add main");
    cx.update_window(handle, |_, window, cx| {
        tab.update(cx, |t, cx| live::refresh_tab(t, window, cx))
    })
    .expect("the window is open");
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let seq = cx.update(|cx| tab.read(cx).opened.iteration.as_ref().map(|it| it.seq));
        if seq == Some(2) {
            break;
        }
    }
    cx.update_window(handle, |_, window, cx| {
        tab.update(cx, |t, cx| iterations::toggle_changes_since(t, window, cx))
    })
    .expect("the window is open");
    let mut settled = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let (since, debug) = cx.update(|cx| {
            let t = tab.read(cx);
            (
                iterations::changes_since_checked(t),
                t.viewport.read(cx).debug(),
            )
        });
        if since
            && debug.visible_rows.len() > 10
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
        {
            settled = true;
            break;
        }
    }
    assert!(settled, "the changes since the review showed");
    let paths: Vec<String> = cx.update(|cx| {
        tab.read(cx)
            .opened
            .files
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect()
    });
    assert_eq!(paths, ["src/config.rs", "src/main.rs"]);

    Shown {
        _tab: tab,
        handle,
        cx,
        _repo: repo,
    }
}
