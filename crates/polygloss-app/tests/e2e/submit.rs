//! Screenshots of T3.10 (design §8.3, §8.7): the composer and the Submit
//! review dialog, as the app draws them at 1280×800 in Polygloss Light.
//!
//! - `e2e_submit_dialog`: two drafts (a line comment and a review-level
//!   comment) and an agent question; the dialog over the tab with a summary,
//!   "Request changes" chosen, the draft count and the waiter line (the
//!   review is assigned to claude-code, which is not waiting).
//! - `e2e_composer_line`: a line composer under `src/config.rs` line 12
//!   holding a comment, below an agent question whose card ends with the
//!   "Reply…" box and "Resolve conversation" (unified).

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::composer::{self, ComposerKey};
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::settings::Settings;
use polygloss_app::settings::model::{LayoutSetting, ThemeMode};
use polygloss_app::submit;
use polygloss_app::tabs::TabItem;
use polygloss_app::threads;
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AssignedBy, Author, AuthorKind, Core, NewThread, OpenRequest, OpenedDiff, SessionInfo, Subject,
    ThreadKind, Verdict,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{Sandbox, code_change_repo};

pub const TESTS: &[Test] = &crate::tests![e2e_submit_dialog, e2e_composer_line];

/// Frames drawn at most while waiting for loads, threads and highlights.
const MAX_FRAMES: usize = 40;

fn author(kind: AuthorKind) -> Author {
    Author {
        kind,
        name: match kind {
            AuthorKind::Human => "you".into(),
            AuthorKind::Agent => "claude-code".into(),
        },
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

fn create(
    core: &Core,
    opened: &OpenedDiff,
    subject: Subject,
    kind: ThreadKind,
    body: &str,
    by: AuthorKind,
) -> String {
    let blobs = BlobReader::open(&opened.repo).expect("open the object store");
    core.create_thread(
        &NewThread {
            review_id: opened.review_id.clone(),
            diff_id: opened.diff_id.clone(),
            subject,
            kind,
            body_md: body.into(),
            author: author(by),
        },
        &blobs,
    )
    .expect("create the thread")
}

fn compare(repo: &std::path::Path) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// Starts the app (Polygloss Light, `layout`) and opens `req`; draws until the
/// tab has settled.
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
    // gpui-component's dialog entrance animation runs on wall-clock time
    // (`dialog::ANIMATION_DURATION`), so a capture a few frames after
    // opening catches it mid-slide at a different offset every run. With
    // reduced motion it draws in place on its first frame.
    cx.update(|cx| cx.set_reduce_motion(true));
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
            for _ in 0..6 {
                screenshot::draw(cx, handle);
            }
            return tab;
        }
    }
    panic!("the review tab never settled");
}

fn e2e_submit_dialog() {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let req = compare(repo.path());
    let opened = core.open(&req).expect("open the review");
    create(
        &core,
        &opened,
        line("src/config.rs", Side::New, 1, 1),
        ThreadKind::Question,
        "Why `BTreeMap` here? Do callers rely on **sorted** keys?",
        AuthorKind::Agent,
    );
    create(
        &core,
        &opened,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Comment,
        "Document the ordering on the field.",
        AuthorKind::Human,
    );
    create(
        &core,
        &opened,
        Subject::Review,
        ThreadKind::Comment,
        "The map change is right; two nits inline.",
        AuthorKind::Human,
    );
    let session = core
        .upsert_session(&SessionInfo {
            id: "session-1".into(),
            client_name: "claude-code".into(),
            client_version: None,
            owner_pid: None,
            cwd: None,
        })
        .unwrap();
    core.assign_review(&opened.review_id, &session, AssignedBy::OpenDiff)
        .unwrap();
    let (mut cx, handle, tab) = app(&sb, LayoutSetting::Unified, core, req);
    cx.update_window(handle, |_, window, cx| {
        tab.update(cx, |t, cx| submit::open_dialog(t, window, cx))
    })
    .unwrap();
    for _ in 0..4 {
        screenshot::draw(&mut cx, handle);
    }
    let dialog = cx
        .update(|cx| submit::dialog(tab.read(cx)))
        .expect("the dialog opened");
    cx.update_window(handle, |_, window, cx| {
        let input = dialog.read(cx).summary_input().clone();
        input.update(cx, |s, cx| {
            s.set_value(
                "Close: the ordering question needs an answer before this lands.",
                window,
                cx,
            )
        });
        dialog.update(cx, |d, cx| d.set_verdict(Verdict::RequestChanges, cx));
    })
    .unwrap();
    // (Reduced motion: the dialog is in place from its first frame.)
    for _ in 0..6 {
        screenshot::draw(&mut cx, handle);
    }
    let (drafts, waiter) = cx.update(|cx| {
        let d = dialog.read(cx);
        (d.drafts(), d.waiter().text())
    });
    assert_eq!(drafts, 2);
    assert_eq!(
        waiter,
        "claude-code isn't listening; it will see this on its next turn"
    );
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

fn e2e_composer_line() {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let core = Core::open_default().expect("open the sandbox store");
    let req = compare(repo.path());
    let opened = core.open(&req).expect("open the review");
    create(
        &core,
        &opened,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Question,
        "Is the map meant to stay sorted when entries are added later?",
        AuthorKind::Agent,
    );
    let (mut cx, handle, tab) = app(&sb, LayoutSetting::Unified, core, req);
    let key = ComposerKey::line("src/config.rs", Side::New, 11, 11);
    cx.update_window(handle, |_, window, cx| {
        tab.update(cx, |t, cx| {
            composer::open_line(t, 0, Side::New, 11, 11, window, cx);
            let view = composer::composer(t, &key, cx).expect("the composer opened");
            let input = view.read(cx).input().clone();
            input.update(cx, |s, cx| {
                s.set_value(
                    "Nit: `entries.insert` returns the old value;\nworth a `debug_assert!` that keys are unique?",
                    window,
                    cx,
                )
            });
        })
    })
    .unwrap();
    for _ in 0..8 {
        screenshot::draw(&mut cx, handle);
    }
    let blocks = cx.update(|cx| tab.read(cx).viewport.read(cx).document().blocks(0).len());
    assert_eq!(blocks, 2, "the question and the composer");
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
