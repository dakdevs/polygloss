//! Screenshots of T3.9 (design §8, §11.6 "Threads"): thread blocks in the
//! diff and the threads panel, as the app draws them at 1280×800 in Polygloss
//! Light.
//!
//! - `e2e_threads_split` / `e2e_threads_unified`: on `src/config.rs`, an
//!   agent question with a draft reply (first line), an old-side draft
//!   (split: left column with a spacer on the right), a draft with a
//!   ` ```suggestion ` (mini-diff), and an agent note collapsed to a chip;
//!   unified shows the panel (hidden by default), which lists them with a
//!   review-level comment and a resolved thread.
//! - `e2e_threads_outdated`: a thread whose line changed in the next
//!   iteration, inline at the nearest line with the Outdated badge and the
//!   original snippet, and in the panel.

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::settings::Settings;
use polygloss_app::settings::model::{LayoutSetting, ThemeMode};
use polygloss_app::tabs::TabItem;
use polygloss_app::threads;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, OpenedDiff, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{CONFIG_RS_HEAD, FixtureRepo, Sandbox, code_change_repo};

pub const TESTS: &[Test] =
    &crate::tests![e2e_threads_split, e2e_threads_unified, e2e_threads_outdated];

/// Frames drawn at most while waiting for loads, threads and highlights.
const MAX_FRAMES: usize = 40;

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
    }
}

fn line(path: &str, side: Side, start_line: u32, line: u32) -> Subject {
    Subject::Line {
        path: path.into(),
        side,
        start_line,
        line,
    }
}

fn compare(repo: &FixtureRepo, head: &str) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            // Full names: on a case-insensitive disk `head` alone is HEAD.
            base: "refs/tags/base".into(),
            head: head.into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn create(
    core: &Core,
    opened: &OpenedDiff,
    subject: Subject,
    kind: ThreadKind,
    body: &str,
    author: Author,
) -> String {
    let blobs = BlobReader::open(&opened.repo).expect("open the object store");
    core.create_thread(
        &NewThread {
            review_id: opened.review_id.clone(),
            diff_id: opened.diff_id.clone(),
            subject,
            kind,
            body_md: body.into(),
            author,
        },
        &blobs,
    )
    .expect("create the thread")
}

/// The threads of the split and unified screenshots.
fn review_threads(core: &Core, opened: &OpenedDiff) {
    let question = create(
        core,
        opened,
        line("src/config.rs", Side::New, 1, 1),
        ThreadKind::Question,
        "Why `BTreeMap` here? Do callers rely on **sorted** keys?",
        agent(),
    );
    core.reply(
        &question,
        "Yes: `polygloss settings` prints them in order.",
        &human(),
    )
    .expect("reply");
    create(
        core,
        opened,
        line("src/config.rs", Side::Old, 1, 1),
        ThreadKind::Comment,
        "`HashMap` was never iterated in order, so this was flaky.",
        human(),
    );
    create(
        core,
        opened,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Comment,
        "Document the ordering:\n\n```suggestion\n    /// Sorted by key.\n    entries: BTreeMap<String, String>,\n```",
        human(),
    );
    create(
        core,
        opened,
        line("src/config.rs", Side::New, 10, 10),
        ThreadKind::Note,
        "The parse loop is unchanged; only the map type moved.",
        agent(),
    );
    create(
        core,
        opened,
        Subject::Review,
        ThreadKind::Comment,
        "I switched the config map to `BTreeMap` so the output is stable.",
        agent(),
    );
    let resolved = create(
        core,
        opened,
        line("src/greet.ts", Side::New, 3, 3),
        ThreadKind::Comment,
        "The fallback greeting could come from the settings.",
        agent(),
    );
    core.set_resolved(&resolved, true, &Actor::human(), None)
        .expect("resolve");
}

/// Starts the app with `layout` in the sandbox's settings and opens `req`.
fn app(
    sb: &Sandbox,
    layout: LayoutSetting,
    core: Core,
    req: OpenRequest,
) -> (HeadlessAppContext, AnyWindowHandle, Entity<ReviewTab>) {
    let mut settings = Settings::default();
    settings.theme.mode = ThemeMode::Light;
    settings.diff.layout = layout;
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
    let tab = settle(&mut cx, handle, &main);
    (cx, handle, tab)
}

/// Draws until the review tab shows highlighted rows and its thread
/// blocks; returns the tab.
fn settle(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    main: &Entity<MainWindow>,
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
        let loaded =
            cx.update(|cx| threads::threads(tab.read(cx)).is_some_and(|m| m.read(cx).is_loaded()));
        let debug = cx.update(|cx| tab.read(cx).viewport.read(cx).debug());
        if loaded
            && debug.visible_rows.len() > 5
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.visible_rows.iter().any(|r| r.starts_with("[block "))
            && debug.styled_rows > 0
        {
            // Comment bodies parse on the background executor and blocks
            // are measured as they paint: a few more frames.
            for _ in 0..6 {
                screenshot::draw(cx, handle);
            }
            return tab;
        }
    }
    let rows = cx.update(|cx| {
        main.read(cx)
            .tabs()
            .get(1)
            .and_then(TabItem::review)
            .map(|t| t.read(cx).viewport.read(cx).debug())
    });
    panic!("the review tab never settled: {rows:#?}");
}

/// Shows the threads panel (hidden by default) and lets it draw.
fn show_panel(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, tab: &Entity<ReviewTab>) {
    cx.update(|cx| tab.update(cx, |t, cx| t.set_threads_panel_visible(true, cx)));
    for _ in 0..6 {
        screenshot::draw(cx, handle);
    }
}

fn e2e_threads_split() {
    capture_review(LayoutSetting::Split);
}

fn e2e_threads_unified() {
    capture_review(LayoutSetting::Unified);
}

fn capture_review(layout: LayoutSetting) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let req = compare(&repo, "refs/tags/head");
    let opened = core.open(&req).expect("open the review");
    review_threads(&core, &opened);
    let (mut cx, handle, tab) = app(&sb, layout, core, req);
    let blocks = cx.update(|cx| tab.read(cx).viewport.read(cx).document().blocks(0).len());
    assert_eq!(blocks, 4, "four threads on src/config.rs");
    if layout == LayoutSetting::Unified {
        // The unified capture shows the panel; split leaves it hidden (the
        // default), as someone reviewing split would.
        show_panel(&mut cx, handle, &tab);
    }
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

fn e2e_threads_outdated() {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    repo.git(&["checkout", "-q", "-b", "topic", "refs/tags/head"]);
    let core = Core::open_default().expect("open the sandbox store");
    let req = compare(&repo, "refs/heads/topic");
    let first = core.open(&req).expect("open iteration 1");
    create(
        &core,
        &first,
        line("src/config.rs", Side::New, 10, 10),
        ThreadKind::Comment,
        "Pre-size this map? We know the line count up front.",
        agent(),
    );
    create(
        &core,
        &first,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Comment,
        "This line did not change, so the thread moved with it.",
        human(),
    );
    let changed = CONFIG_RS_HEAD.replace(
        "let mut entries = BTreeMap::new();",
        "let mut entries: BTreeMap<String, String> = BTreeMap::new();",
    );
    repo.write("src/config.rs", changed.as_bytes());
    repo.commit("spell out the map type");
    let (mut cx, handle, tab) = app(&sb, LayoutSetting::Unified, core, req);
    let outdated = cx.update(|cx| {
        let m = threads::threads(tab.read(cx)).unwrap().read(cx);
        m.threads()
            .filter(|t| {
                m.position(&t.id).map(|p| p.state)
                    == Some(polygloss_core::review::PositionState::Outdated)
            })
            .count()
    });
    assert_eq!(outdated, 1);
    show_panel(&mut cx, handle, &tab);
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
