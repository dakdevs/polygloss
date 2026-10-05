//! Screenshot of T3.13 (design §11.7, §17): a review the agent worked on
//! while the human was away. The banner strip shows "claude-code replied to
//! 2 threads" with its Show button and "Re-review requested" with the
//! summary's first line and View changes; the agent's question and note
//! are in the diff and the threads panel.

use std::sync::Arc;

use gpui_kit::{px, size};
use polygloss_app::review_tab::{BannerKind, open_review};
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
use polygloss_core::store::events::{Actor, ActorKind};
use polygloss_diff::Side;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{Sandbox, code_change_repo};

pub const TESTS: &[Test] = &crate::tests![e2e_feed_banners];

/// Frames drawn at most while waiting for loads, threads and banners.
const MAX_FRAMES: usize = 40;

fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
    }
}

fn e2e_feed_banners() {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
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
    let opened = core.open(&req).expect("open the review");
    let blobs = BlobReader::open(&opened.repo).expect("open the object store");
    let thread = |subject: Subject, kind: ThreadKind, body: &str| {
        core.create_thread(
            &NewThread {
                review_id: opened.review_id.clone(),
                diff_id: opened.diff_id.clone(),
                subject,
                kind,
                body_md: body.into(),
                author: agent(),
            },
            &blobs,
        )
        .expect("create the thread")
    };
    thread(
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 1,
            line: 1,
        },
        ThreadKind::Question,
        "Should `Config` keep insertion order instead? `BTreeMap` sorts by key.",
    );
    thread(
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 10,
            line: 10,
        },
        ThreadKind::Note,
        "The parse loop is unchanged; only the map type moved.",
    );
    core.request_rereview(
        &opened.review_id,
        "Switched `Config` to **BTreeMap** and added `is_empty`, as asked.\n\nThe tests cover both.",
        &Actor {
            kind: ActorKind::Agent,
            name: Some("claude-code".into()),
            session_id: None,
        },
        None,
    )
    .expect("request a re-review");

    let mut settings = Settings::default();
    settings.theme.mode = ThemeMode::Light;
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
        let Some(tab) = tab else { continue };
        let loaded =
            cx.update(|cx| threads::threads(tab.read(cx)).is_some_and(|m| m.read(cx).is_loaded()));
        let banners = cx.update(|cx| tab.read(cx).banners.read(cx).banners());
        let debug = cx.update(|cx| tab.read(cx).viewport.read(cx).debug());
        if loaded
            && banners.len() == 2
            && debug.visible_rows.len() > 5
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.visible_rows.iter().any(|r| r.starts_with("[block "))
            && debug.styled_rows > 0
        {
            assert_eq!(
                banners,
                [
                    (
                        BannerKind::AgentReplies,
                        "claude-code replied to 2 threads".into()
                    ),
                    (
                        BannerKind::Rereview,
                        "claude-code requested a re-review: Switched Config to BTreeMap and \
                         added is_empty, as asked."
                            .into()
                    ),
                ]
            );
            settled = true;
            break;
        }
    }
    assert!(settled, "the review tab settled with both banners");
    // Comment bodies parse on the background executor and blocks are
    // measured as they paint: a few more frames.
    for _ in 0..6 {
        screenshot::draw(&mut cx, handle);
    }
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
