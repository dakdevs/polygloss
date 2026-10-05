//! Screenshots of T6.13 (design §11.6, §11.7, ADR-0027): the header card
//! above the first file card. A commit's (avatar, subject, author and time,
//! stats, SHA); a compare's with its "Show commits" list open; and, in
//! Polygloss Dark, a live review's (branch, base, stats, Snapshot) under a
//! "1 file changed · Refresh (R)" notice.

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::live;
use polygloss_app::review_tab::{BannerKind, ReviewTab, open_review};
use polygloss_app::settings::Settings;
use polygloss_app::settings::model::ThemeMode;
use polygloss_app::tabs::TabItem;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![
    e2e_header_card_commit,
    e2e_header_card_compare,
    e2e_live_header_and_banner_dark,
];

/// Frames drawn at most while waiting for loads, highlights and the card.
const MAX_FRAMES: usize = 30;

const MAIN_RS: &str = "mod config;\n\nfn main() {\n    let text = std::fs::read_to_string(\"app.conf\").unwrap_or_default();\n    let config = config::Config::parse(&text);\n    println!(\"{} settings\", config.len());\n}\n";

/// `main`: `src/config.rs`. Branch `feature` (checked out): three commits
/// by two authors, the last one Ada Lovelace's "Parse configs into a
/// BTreeMap".
fn feature_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.commit("Add the config parser");
    repo.branch("feature");
    repo.checkout("feature");
    let commits = [
        (
            "src/main.rs",
            MAIN_RS,
            "Grace Hopper <grace@example.org>",
            "Read app.conf at startup",
        ),
        (
            "README.md",
            "# app\n\nReads `app.conf`.\n",
            "Grace Hopper <grace@example.org>",
            "Document the config file",
        ),
        (
            "src/config.rs",
            CONFIG_RS_HEAD,
            "Ada Lovelace <ada@example.com>",
            "Parse configs into a BTreeMap",
        ),
    ];
    for (path, text, author, subject) in commits {
        repo.write(path, text.as_bytes());
        repo.commit(subject);
        repo.git(&["commit", "-q", "--amend", "--no-edit", "--author", author]);
    }
    repo
}

/// The app at 1280×800 in `mode`, showing `req` once its rows are
/// highlighted and its header card is in.
fn open_settled(
    req: OpenRequest,
    mode: ThemeMode,
    sb: &Sandbox,
) -> (HeadlessAppContext, AnyWindowHandle, Entity<ReviewTab>) {
    let mut settings = Settings::default();
    settings.theme.mode = mode;
    let file = sb.config_dir().join("polygloss/settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_string(&settings).unwrap()).unwrap();
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
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
        let (debug, card) = cx.update(|cx| {
            let v = tab.read(cx).viewport.read(cx);
            (v.debug(), v.document().prelude_height())
        });
        if debug.visible_rows.len() > 10
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
            && card.is_some_and(|h| h > 0.0)
        {
            return (cx, handle, tab);
        }
    }
    panic!("the review tab never settled");
}

fn capture(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) {
    for _ in 0..3 {
        screenshot::draw(cx, handle);
    }
    let image = screenshot::capture(cx, handle);
    assert_screenshot(&image);
}

fn e2e_header_card_commit() {
    let sb = Sandbox::isolate();
    let repo = feature_repo();
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit {
            rev: "refs/heads/feature".into(),
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let (mut cx, handle, _tab) = open_settled(req, ThemeMode::Light, &sb);
    capture(&mut cx, handle);
}

fn e2e_header_card_compare() {
    let sb = Sandbox::isolate();
    let repo = feature_repo();
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
    let (mut cx, handle, _tab) = open_settled(req, ThemeMode::Light, &sb);
    cx.update_window(handle, |_, window, cx| {
        window.click("header-commits-toggle", cx);
    })
    .expect("the window is open");
    screenshot::park_pointer(&mut cx, handle);
    capture(&mut cx, handle);
}

fn e2e_live_header_and_banner_dark() {
    let sb = Sandbox::isolate();
    let repo = feature_repo();
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Live {
            since: Since::MergeBase,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let (mut cx, handle, tab) = open_settled(req, ThemeMode::Dark, &sb);
    // An agent adds a file; the recompute (what the watcher runs after a
    // batch) finds it.
    repo.write("src/lib.rs", b"pub mod config;\n");
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
    capture(&mut cx, handle);
}
