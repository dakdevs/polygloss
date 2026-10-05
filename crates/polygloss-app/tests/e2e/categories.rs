//! Screenshots of T6.14 (design §11.6 "Sections", §11.15): a review whose
//! test and generated files sit in sections below the main files, at
//! 1280×800 in Polygloss Light and Dark. The main files first, then the
//! Tests section opened from its band (an agent note on its test file),
//! then the Generated section closed with its lockfile.

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::categories;
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::settings::Settings;
use polygloss_app::settings::model::{LayoutSetting, ThemeMode};
use polygloss_app::tabs::TabItem;
use polygloss_app::threads;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::ViewportEvent;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_categories_sections, e2e_categories_sections_dark];

/// Frames drawn at most while waiting for loads, threads and highlights.
const MAX_FRAMES: usize = 40;

const PARSE_RS_BASE: &str = "pub fn parse(text: &str) -> Vec<(String, String)> {\n    text.lines()\n        .filter_map(|l| l.split_once('='))\n        .map(|(k, v)| (k.to_string(), v.to_string()))\n        .collect()\n}\n";

const PARSE_RS_HEAD: &str = "pub fn parse(text: &str) -> Vec<(String, String)> {\n    text.lines()\n        .map(str::trim)\n        .filter(|l| !l.starts_with('#'))\n        .filter_map(|l| l.split_once('='))\n        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))\n        .collect()\n}\n";

const MAIN_RS: &str = "mod parse;\n\nfn main() {\n    parse::run();\n}\n";

const PARSE_TEST_RS: &str = "use app::parse::parse;\n\n#[test]\nfn skips_comments() {\n    assert!(parse(\"# note\").is_empty());\n}\n";

const LOCK_BASE: &str = "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n";
const LOCK_HEAD: &str = "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.2.0\"\n";

/// Tags `base` and `head`; the diff, in order: `Cargo.lock` (Generated),
/// `src/main.rs` (added), `src/parse.rs` (modified), `tests/parse.rs`
/// (added, Tests).
fn sections_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("Cargo.lock", LOCK_BASE.as_bytes());
    repo.write("src/parse.rs", PARSE_RS_BASE.as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("Cargo.lock", LOCK_HEAD.as_bytes());
    repo.write("src/main.rs", MAIN_RS.as_bytes());
    repo.write("src/parse.rs", PARSE_RS_HEAD.as_bytes());
    repo.write("tests/parse.rs", PARSE_TEST_RS.as_bytes());
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

fn e2e_categories_sections() {
    capture(ThemeMode::Light);
}

fn e2e_categories_sections_dark() {
    capture(ThemeMode::Dark);
}

fn capture(mode: ThemeMode) {
    let sb = Sandbox::isolate();
    let repo = sections_repo();
    let core = Core::open_default().expect("open the sandbox store");
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
    let opened = core.open(&req).expect("open the review");
    let blobs = BlobReader::open(&opened.repo).expect("open the object store");
    core.create_thread(
        &NewThread {
            review_id: opened.review_id.clone(),
            diff_id: opened.diff_id.clone(),
            subject: Subject::Line {
                path: "tests/parse.rs".into(),
                side: Side::New,
                start_line: 4,
                line: 4,
            },
            kind: ThreadKind::Note,
            body_md: "Covers the comment lines `parse` now skips.".into(),
            author: Author {
                kind: AuthorKind::Agent,
                name: "claude-code".into(),
                session_id: None,
            },
        },
        &blobs,
    )
    .expect("create the note");

    let mut settings = Settings::default();
    settings.theme.mode = mode;
    settings.diff.layout = LayoutSetting::Unified;
    let file = sb.config_dir().join("polygloss/settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_string(&settings).unwrap()).unwrap();
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
    let tab = settle(
        &mut cx,
        handle,
        |cx, tab| {
            let loaded = cx.update(|cx| {
                threads::threads(tab.read(cx)).is_some_and(|m| m.read(cx).is_loaded())
            });
            let card = cx.update(|cx| tab.read(cx).viewport.read(cx).document().prelude_height());
            loaded && card.is_some_and(|h| h > 0.0)
        },
        &main,
    );
    let tests = cx.update(|cx| {
        categories::partition(tab.read(cx))
            .expect("partitioned")
            .sections
            .iter()
            .find(|s| s.id.to_string() == "tests")
            .expect("a Tests section")
            .viewport_id
    });

    // Open Tests as its band's Show link does (the app tests click the
    // band itself; a headless window has no element to aim at here).
    cx.update(|cx| {
        let viewport = tab.read(cx).viewport.clone();
        viewport.update(cx, |v, cx| {
            v.set_section_open(tests, true, cx);
            cx.emit(ViewportEvent::SectionToggled {
                id: tests,
                open: true,
            });
        });
    });
    screenshot::park_pointer(&mut cx, handle);
    let tab = settle(
        &mut cx,
        handle,
        |cx, tab| {
            let open = cx.update(|cx| tab.read(cx).viewport.read(cx).section_open(tests));
            open == Some(true)
        },
        &main,
    );
    // The end of the review: the main files, the test file's card and the
    // closed Generated band below it.
    cx.update(|cx| {
        let viewport = tab.read(cx).viewport.clone();
        viewport.update(cx, |v, cx| v.scroll_by(10_000.0, cx));
    });
    for _ in 0..6 {
        screenshot::draw(&mut cx, handle);
    }
    let rows = cx.update(|cx| tab.read(cx).viewport.read(cx).debug().visible_rows);
    assert!(rows.iter().any(|r| r == "▾ 1 test file"), "{rows:#?}");
    assert!(rows.iter().any(|r| r == "▸ 1 generated file"), "{rows:#?}");
    for _ in 0..3 {
        screenshot::draw(&mut cx, handle);
    }
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

/// Draws until the review tab's rows are loaded and highlighted and `ready`
/// holds; returns the tab.
fn settle(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    ready: impl Fn(&mut HeadlessAppContext, &Entity<ReviewTab>) -> bool,
    main: &Entity<polygloss_app::window::MainWindow>,
) -> Entity<ReviewTab> {
    for _ in 0..MAX_FRAMES {
        screenshot::draw(cx, handle);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        let Some(tab) = tab else { continue };
        let debug = cx.update(|cx| tab.read(cx).viewport.read(cx).debug());
        if debug.visible_rows.len() > 5
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
            && ready(cx, &tab)
        {
            for _ in 0..6 {
                screenshot::draw(cx, handle);
            }
            return tab;
        }
    }
    panic!("the review tab never settled");
}
