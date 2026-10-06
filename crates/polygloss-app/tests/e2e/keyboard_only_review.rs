//! A whole review with the keyboard alone (T5.6, OQ-23), in the app as it
//! starts, on the real text system: ⌘O → the repo → Working tree; `n`/`p`,
//! `j`/`k`, `v`, `c` + text + `⌘⏎`, `⌘⇧⏎` with a summary, `⇥` out of it
//! and the verdict picked with `⌘2`, then `R` after the working tree
//! changed. Only key events reach
//! the window (no mouse events, not even a pointer move).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, Keystroke, px, size};
use polygloss_app::composer::{self, ComposerKey};
use polygloss_app::keyboard::{self, Pane};
use polygloss_app::review_tab::{BannerKind, ReviewTab};
use polygloss_app::tabs::TabItem;
use polygloss_app::window::MainWindow;
use polygloss_app::{live, open_flow, startup, window};
use polygloss_core::git::Source;
use polygloss_core::review::{Core, OpenRequest, Verdict};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_keyboard_only_review];

/// Frames drawn at most while waiting for something.
const MAX_FRAMES: usize = 60;

fn numbered(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("fn {prefix}_{i}() {{}}\n"))
        .collect()
}

/// Types `keys` (space-separated GPUI keystrokes) into the window, a frame
/// after each.
fn press(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, keys: &str) {
    for key in keys.split_whitespace() {
        let keystroke = Keystroke::parse(key).expect("a keystroke");
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        })
        .expect("the window is open");
        screenshot::draw(cx, handle);
    }
}

/// Types `text` character by character.
fn type_text(cx: &mut HeadlessAppContext, handle: AnyWindowHandle, text: &str) {
    for c in text.chars() {
        let key = c.to_string();
        let keystroke = Keystroke {
            modifiers: Default::default(),
            key: key.clone(),
            key_char: Some(key),
        };
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        })
        .expect("the window is open");
    }
    screenshot::draw(cx, handle);
}

/// Draws frames until `done` holds.
fn wait_for(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    what: &str,
    mut done: impl FnMut(&mut HeadlessAppContext) -> bool,
) {
    for _ in 0..MAX_FRAMES {
        screenshot::draw(cx, handle);
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn active_review(
    cx: &mut HeadlessAppContext,
    main: &Entity<MainWindow>,
) -> Option<Entity<ReviewTab>> {
    cx.update(|cx| {
        let tabs = main.read(cx).tabs();
        tabs.get(tabs.active()).and_then(TabItem::review).cloned()
    })
}

fn e2e_keyboard_only_review() {
    let _sb = Sandbox::isolate();
    // A feature branch with two files changed in the working tree.
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/a.rs", numbered("a", 40).as_bytes());
    repo.write("src/b.rs", numbered("b", 40).as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write(
        "src/a.rs",
        numbered("a", 40)
            .replace("fn a_5() {}", "fn a_5() { edited(); }")
            .as_bytes(),
    );
    repo.write(
        "src/b.rs",
        numbered("b", 40)
            .replace("fn b_7() {}", "fn b_7() { edited(); }")
            .as_bytes(),
    );
    // The repo is a recent one (reviewed before), so ⌘O lists it.
    let core = Core::open_default().expect("open the sandbox store");
    core.open(&OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit { rev: "HEAD".into() },
        label: None,
        pin: None,
        actor: Actor::human(),
    })
    .expect("an earlier review of the repo");

    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    cx.update(|cx| {
        polygloss_app::motion::set_override(Some(polygloss_app::motion::MotionPolicy::Off), cx)
    });
    let (handle, main) = cx.update(|cx| {
        startup::init(core.clone(), cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::draw(&mut cx, handle);

    // ⌘O, the repo (the only recent one), Working tree (⌘1), open (⌘⏎).
    press(&mut cx, handle, "cmd-o");
    let flow = cx
        .update(|cx| open_flow::current(cx))
        .expect("⌘O opened the flow");
    wait_for(&mut cx, handle, "the repo list", |cx| {
        cx.update(|cx| {
            let list = flow.read(cx).repos().clone();
            list.read(cx).delegate().matches().next().is_some()
        })
    });
    press(&mut cx, handle, "enter");
    wait_for(&mut cx, handle, "the source step", |cx| {
        cx.update(|cx| flow.read(cx).source().is_some())
    });
    press(&mut cx, handle, "cmd-1 cmd-enter");
    wait_for(&mut cx, handle, "the live review tab", |cx| {
        active_review(cx, &main).is_some_and(|t| {
            cx.update(|cx| {
                let t = t.read(cx);
                t.opened.review_key.starts_with("worktree:")
                    && t.viewport.read(cx).debug().visible_rows.len() > 5
            })
        })
    });
    let tab = active_review(&mut cx, &main).expect("the live tab");
    let cursor =
        |cx: &mut HeadlessAppContext| cx.update(|cx| tab.read(cx).viewport.read(cx).cursor());
    let pane = |cx: &mut HeadlessAppContext| {
        cx.update_window(handle, |_, window, cx| {
            keyboard::focused_pane(tab.read(cx), window, cx)
        })
        .unwrap()
    };
    assert_eq!(
        pane(&mut cx),
        Some(Pane::Viewport),
        "the diff has the keyboard"
    );

    // j / k move the line cursor; n / p go between files.
    press(&mut cx, handle, "j j");
    let first = cursor(&mut cx).expect("j puts the cursor in the diff");
    press(&mut cx, handle, "k");
    let up = cursor(&mut cx).unwrap();
    assert_eq!(up.file_idx, first.file_idx);
    assert!(up.line < first.line || up.side != first.side, "k moved up");
    press(&mut cx, handle, "n");
    assert_eq!(cursor(&mut cx).map(|c| c.file_idx), Some(1), "n: next file");
    press(&mut cx, handle, "p");
    assert_eq!(
        cursor(&mut cx).map(|c| c.file_idx),
        Some(0),
        "p: previous file"
    );

    // v: `a.rs` is viewed, collapsed, and the cursor is on to `b.rs`.
    press(&mut cx, handle, "v");
    wait_for(&mut cx, handle, "a.rs viewed", |cx| {
        cx.update(|cx| {
            let v = tab.read(cx).viewport.read(cx);
            v.file_flags().first().is_some_and(|f| f.viewed) && v.collapsed() == vec![0]
        })
    });

    // c on `b.rs`'s changed line, a comment, ⌘⏎: a draft.
    press(&mut cx, handle, "n");
    press(&mut cx, handle, "]");
    let at = cursor(&mut cx).expect("the cursor on b.rs's change");
    assert_eq!(at.file_idx, 1);
    press(&mut cx, handle, "c");
    let key = ComposerKey::Line {
        path: "src/b.rs".into(),
        side: at.side,
        start_line: at.line + 1,
        line: at.line + 1,
    };
    assert!(
        cx.update(|cx| composer::composer(tab.read(cx), &key, cx).is_some()),
        "c opened a composer"
    );
    assert_eq!(pane(&mut cx), Some(Pane::Composer(key.clone())));
    type_text(&mut cx, handle, "Why edited?");
    press(&mut cx, handle, "cmd-enter");
    let review = cx.update(|cx| tab.read(cx).review_id.clone());
    wait_for(&mut cx, handle, "the draft", |_| {
        core.drafts_count(&review).unwrap() == 1
    });

    // ⌘⇧⏎: the Submit review dialog, the summary typed, Approve, ⌘⏎
    // submits.
    press(&mut cx, handle, "cmd-shift-enter");
    wait_for(&mut cx, handle, "the submit dialog", |cx| {
        cx.update(|cx| polygloss_app::submit::dialog(tab.read(cx)).is_some())
    });
    type_text(&mut cx, handle, "One question");
    // ⇥ leaves the summary for the dialog's controls (no indent typed);
    // ⌘2 picks Approve.
    press(&mut cx, handle, "tab");
    let dialog = cx
        .update(|cx| polygloss_app::submit::dialog(tab.read(cx)))
        .expect("the dialog is open");
    let summary_focused = cx
        .update_window(handle, |_, window, cx| {
            dialog.read(cx).summary_focus(cx).is_focused(window)
        })
        .unwrap();
    assert!(!summary_focused, "⇥ left the summary");
    press(&mut cx, handle, "cmd-2");
    assert_eq!(cx.update(|cx| dialog.read(cx).verdict()), Verdict::Approve);
    assert_eq!(cx.update(|cx| dialog.read(cx).summary(cx)), "One question");
    press(&mut cx, handle, "cmd-enter");
    wait_for(&mut cx, handle, "the submission", |_| {
        core.last_submission(&review)
            .unwrap()
            .is_some_and(|s| s.verdict == Verdict::Approve)
    });
    assert_eq!(core.drafts_count(&review).unwrap(), 0, "the draft went out");

    // The working tree changes; the banner offers the refresh; R applies it.
    repo.write("src/c.rs", b"fn c() {}\n");
    cx.update(|cx| tab.update(cx, live::recompute_now));
    wait_for(&mut cx, handle, "the live banner", |cx| {
        cx.update(|cx| {
            tab.read(cx)
                .banners
                .read(cx)
                .banners()
                .iter()
                .any(|(k, _)| *k == BannerKind::LiveChanges)
        })
    });
    // Back in the diff after the dialog closed.
    assert_eq!(pane(&mut cx), Some(Pane::Viewport));
    press(&mut cx, handle, "shift-r");
    wait_for(&mut cx, handle, "the refreshed diff", |cx| {
        cx.update(|cx| tab.read(cx).opened.files.len() == 3)
    });
    let banners = cx.update(|cx| tab.read(cx).banners.read(cx).banners());
    assert!(
        banners.iter().all(|(k, _)| *k != BannerKind::LiveChanges),
        "the banner went with the refresh"
    );
}
