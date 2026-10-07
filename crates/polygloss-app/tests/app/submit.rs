//! GPUI tests of T3.10: the Submit review dialog (design §8.3, §11.4,
//! ADR-0011): `⌘⇧⏎` and the toolbar button (with the draft count) open it;
//! it publishes every draft with the verdict in one step, allows zero
//! drafts, autosaves its summary and verdict, and says whether the agent is
//! listening.

use std::time::Duration;

use gpui_kit::{Entity, VisualTestContext};
use polygloss_app::motion::{self, MotionPolicy};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::submit::{self, SubmitDialog, WaiterState, dialog::AUTOSAVE_DEBOUNCE};
use polygloss_app::threads;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AssignedBy, Author, AuthorKind, NewThread, SessionInfo, Subject, SubmitDraft, ThreadFilter,
    ThreadKind, ThreadScope, Verdict, Viewer,
};
use polygloss_diff::Side;

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

fn painted(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .is_some()
}

fn click(shell: &mut Shell, selector: &str) {
    let bounds = shell
        .cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} was not painted"));
    shell
        .cx
        .simulate_click(bounds.center(), gpui_kit::Modifiers::none());
    draw(shell.cx);
}

fn draft(shell: &mut Shell, tab: &Entity<ReviewTab>, subject: Subject, body: &str) -> String {
    let (review_id, diff_id, repo) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo).unwrap();
    shell
        .core
        .create_thread(
            &NewThread {
                review_id,
                diff_id,
                subject,
                kind: ThreadKind::Comment,
                body_md: body.into(),
                author: human(),
            },
            &blobs,
        )
        .unwrap()
}

fn two_drafts(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    draft(
        shell,
        tab,
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 5,
            line: 5,
        },
        "Why sorted?",
    );
    draft(shell, tab, Subject::Review, "Nice cleanup.");
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
}

fn dialog(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<Entity<SubmitDialog>> {
    tab.read_with(shell.cx, |t, _| submit::dialog(t))
}

/// Opens the dialog with ⌘⇧⏎ from the diff, with motion Off: gpui-kit's
/// dialog slides in for 250 ms on the wall clock, so on a loaded machine a
/// click's mouse up lands after its button moved and the click is lost.
fn open(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Entity<SubmitDialog> {
    shell
        .cx
        .update(|_, cx| motion::set_override(Some(MotionPolicy::Off), cx));
    let focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    shell.cx.update(|window, cx| window.focus(&focus, cx));
    shell.cx.simulate_keystrokes("cmd-shift-enter");
    draw(shell.cx);
    let d = dialog(shell, tab).expect("the dialog opened");
    assert!(painted(shell.cx, "submit-dialog"));
    d
}

fn review_status(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    shell
        .core
        .store
        .read(|c| {
            Ok(
                c.query_row("SELECT status FROM reviews WHERE id = ?1", [&review], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap()
}

#[gpui_kit::test]
fn toolbar_shows_the_draft_count(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert!(painted(shell.cx, "submit-review"));
    assert!(!painted(shell.cx, "submit-review-count"));
    two_drafts(&mut shell, &tab);
    assert_eq!(tab.read_with(shell.cx, submit::drafts_count), 2);
    assert!(painted(shell.cx, "submit-review-count"));
    // The button opens the dialog too.
    click(&mut shell, "submit-review");
    assert!(dialog(&mut shell, &tab).is_some());
}

#[gpui_kit::test]
fn submit_dialog_publishes_drafts_with_verdict(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    two_drafts(&mut shell, &tab);
    let d = open(&mut shell, &tab);
    assert_eq!(d.read_with(shell.cx, |d, _| d.drafts()), 2);
    assert_eq!(d.read_with(shell.cx, |d, _| d.verdict()), Verdict::Comment);
    // The summary has the keyboard.
    shell.cx.simulate_input("Two questions, see inline.");
    draw(shell.cx);
    click(&mut shell, "submit-verdict-request-changes");
    assert_eq!(
        d.read_with(shell.cx, |d, _| d.verdict()),
        Verdict::RequestChanges
    );
    click(&mut shell, "submit-confirm");
    // Closed; the drafts are published; the review has the verdict.
    assert!(dialog(&mut shell, &tab).is_none());
    assert!(!painted(shell.cx, "submit-dialog"));
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    assert_eq!(shell.core.drafts_count(&review).unwrap(), 0);
    let agent_view = shell
        .core
        .threads(
            ThreadScope::Review(review.clone()),
            Viewer::Agent,
            &ThreadFilter::default(),
        )
        .unwrap();
    assert_eq!(agent_view.len(), 2, "agents see the published threads");
    assert_eq!(review_status(&mut shell, &tab), "changes_requested");
    let summary: String = shell
        .core
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT summary_md || ':' || verdict || ':' || comment_count FROM review_submissions",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(summary, "Two questions, see inline.:request_changes:2");
    // The tab caught up: no drafts, no Draft badges.
    assert_eq!(tab.read_with(shell.cx, submit::drafts_count), 0);
    assert!(!painted(shell.cx, "submit-review-count"));
}

#[gpui_kit::test]
fn submit_with_zero_drafts_allowed(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let d = open(&mut shell, &tab);
    assert_eq!(d.read_with(shell.cx, |d, _| d.drafts()), 0);
    click(&mut shell, "submit-verdict-approve");
    // ⌘⏎ submits, wherever the keyboard is in the dialog.
    shell.cx.simulate_keystrokes("cmd-enter");
    draw(shell.cx);
    assert!(dialog(&mut shell, &tab).is_none());
    assert_eq!(review_status(&mut shell, &tab), "approved");
    let summary: String = shell
        .core
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT summary_md || ':' || comment_count FROM review_submissions",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(summary, ":0");
}

#[gpui_kit::test]
fn submit_dialog_autosaves_summary_and_verdict(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let d = open(&mut shell, &tab);
    shell.cx.simulate_input("Almost there");
    draw(shell.cx);
    click(&mut shell, "submit-verdict-approve");
    // Nothing is written until the typing stops.
    assert_eq!(shell.core.submit_draft(&review).unwrap(), None);
    shell
        .cx
        .executor()
        .advance_clock(AUTOSAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
    assert_eq!(
        shell.core.submit_draft(&review).unwrap(),
        Some(SubmitDraft {
            summary_md: "Almost there".into(),
            verdict: Some(Verdict::Approve)
        })
    );
    assert_eq!(d.read_with(shell.cx, |d, _| d.autosaves()), 1);
    // Esc closes it; opening it again brings both back.
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!painted(shell.cx, "submit-dialog"));
    let d = open(&mut shell, &tab);
    assert_eq!(d.read_with(shell.cx, |d, cx| d.summary(cx)), "Almost there");
    assert_eq!(d.read_with(shell.cx, |d, _| d.verdict()), Verdict::Approve);
}

#[gpui_kit::test]
fn submit_dialog_saves_pending_edits_on_close(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let d = open(&mut shell, &tab);
    shell.cx.simulate_input("Needs work");
    draw(shell.cx);
    click(&mut shell, "submit-verdict-request-changes");
    assert_eq!(d.read_with(shell.cx, |d, _| d.autosaves()), 0);
    // Esc inside the debounce: closing the dialog writes what the debounce
    // had not saved yet.
    drop(d);
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    draw(shell.cx);
    assert!(!painted(shell.cx, "submit-dialog"));
    assert!(
        dialog(&mut shell, &tab).is_none(),
        "the dialog was released"
    );
    assert_eq!(
        shell.core.submit_draft(&review).unwrap(),
        Some(SubmitDraft {
            summary_md: "Needs work".into(),
            verdict: Some(Verdict::RequestChanges)
        })
    );
    let d = open(&mut shell, &tab);
    assert_eq!(d.read_with(shell.cx, |d, cx| d.summary(cx)), "Needs work");
    assert_eq!(
        d.read_with(shell.cx, |d, _| d.verdict()),
        Verdict::RequestChanges
    );
    // The same through Cancel.
    shell.cx.simulate_input(", twice");
    draw(shell.cx);
    drop(d);
    click(&mut shell, "submit-cancel");
    draw(shell.cx);
    assert_eq!(
        shell
            .core
            .submit_draft(&review)
            .unwrap()
            .map(|s| s.summary_md),
        Some("Needs work, twice".into())
    );
}

// Several seeds: the test scheduler runs ready tasks in a seeded random
// order, so without the chaining some seed runs the autosave's write after
// the submission's.
#[gpui_kit::test(iterations = 16)]
fn submit_after_autosave_leaves_no_stale_draft(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let d = open(&mut shell, &tab);
    shell.cx.simulate_input("Ship it");
    click(&mut shell, "submit-verdict-approve");
    // The debounce fires and its write starts but has not run (one task at
    // a time: `advance_clock` would run it to the end); submitting then
    // waits for it, so the submission consumes the saved draft and nothing
    // writes it back afterwards.
    let scheduler = shell.cx.dispatcher.scheduler().clone();
    let mut steps = 0;
    while !d.read_with(shell.cx, |d, _| d.write_in_flight()) {
        if !scheduler.tick() {
            assert!(
                scheduler.advance_clock_to_next_timer(),
                "the autosave never started"
            );
        }
        steps += 1;
        assert!(steps < 10_000, "the autosave never started");
    }
    assert_eq!(d.read_with(shell.cx, |d, _| d.autosaves()), 0);
    d.update(shell.cx, |d, cx| d.submit(cx));
    drop(d);
    draw(shell.cx);
    draw(shell.cx);
    assert!(dialog(&mut shell, &tab).is_none());
    assert_eq!(review_status(&mut shell, &tab), "approved");
    assert_eq!(shell.core.submit_draft(&review).unwrap(), None);
    let d = open(&mut shell, &tab);
    assert_eq!(d.read_with(shell.cx, |d, cx| d.summary(cx)), "");
    assert_eq!(d.read_with(shell.cx, |d, _| d.verdict()), Verdict::Comment);
}

#[gpui_kit::test]
fn edits_while_submitting_are_ignored(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let d = open(&mut shell, &tab);
    shell.cx.simulate_input("Ship it");
    click(&mut shell, "submit-verdict-approve");
    // Edits made after Submit, before the submission lands, neither change
    // what is being submitted nor schedule a write: one would land after
    // the submission consumed the draft (the dialog writes pending edits
    // when it closes) and bring a stale draft back.
    let summary = d.read_with(shell.cx, |d, _| d.summary_input().clone());
    shell.cx.update(|window, cx| {
        d.update(cx, |d, cx| d.submit(cx));
        d.update(cx, |d, cx| d.set_verdict(Verdict::RequestChanges, cx));
        summary.update(cx, |s, cx| s.replace_all("Ship it, then fix", window, cx));
    });
    assert!(d.read_with(shell.cx, |d, _| d.is_submitting()));
    assert_eq!(d.read_with(shell.cx, |d, _| d.verdict()), Verdict::Approve);
    drop((d, summary));
    draw(shell.cx);
    draw(shell.cx);
    shell
        .cx
        .executor()
        .advance_clock(AUTOSAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
    assert!(dialog(&mut shell, &tab).is_none(), "submitted and closed");
    assert_eq!(review_status(&mut shell, &tab), "approved");
    assert_eq!(shell.core.submit_draft(&review).unwrap(), None);
}

#[gpui_kit::test]
fn submit_dialog_shows_waiter_state(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let d = open(&mut shell, &tab);
    assert_eq!(
        d.read_with(shell.cx, |d, _| d.waiter().clone()),
        WaiterState::Unassigned
    );
    assert!(painted(shell.cx, "submit-waiter"));
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);

    // Assigned to claude-code, nothing waiting.
    let session = shell
        .core
        .upsert_session(&SessionInfo {
            id: "s1".into(),
            client_name: "claude-code".into(),
            client_version: None,
            owner_pid: None,
            cwd: None,
        })
        .unwrap();
    shell
        .core
        .assign_review(&review, &session, AssignedBy::OpenDiff)
        .unwrap();
    let d = open(&mut shell, &tab);
    let waiter = d.read_with(shell.cx, |d, _| d.waiter().clone());
    assert_eq!(
        waiter.text(),
        "claude-code isn't listening; it will see this on its next turn"
    );
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);

    // A live waiter (this process stands in for `polygloss wait`).
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 600_000;
    shell
        .core
        .register_waiter(&session, std::process::id() as i32, deadline)
        .unwrap();
    let d = open(&mut shell, &tab);
    let waiter = d.read_with(shell.cx, |d, _| d.waiter().clone());
    assert_eq!(
        waiter,
        WaiterState::Listening {
            agent: "claude-code".into()
        }
    );
    assert_eq!(waiter.text(), "claude-code is listening");
}

/// ⌘1–⌘3 pick the dialog's verdict while it is open; the window's ⌘1–⌘9
/// (the open reviews, T6.6) never reach past it.
#[gpui_kit::test]
fn submit_dialog_keeps_cmd_1_for_its_verdict(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    let tab = shell
        .open(polygloss_core::review::OpenRequest {
            source: polygloss_core::git::Source::Commit {
                rev: "refs/tags/head".into(),
            },
            ..compare_req(repo.path())
        })
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    let d = open(&mut shell, &tab);
    let verdict = |shell: &mut Shell| d.read_with(shell.cx, |d, _| d.verdict());
    for (keys, expected) in [
        ("cmd-3", Verdict::RequestChanges),
        ("cmd-1", Verdict::Comment),
        ("cmd-2", Verdict::Approve),
    ] {
        shell.cx.simulate_keystrokes(keys);
        draw(shell.cx);
        assert_eq!(verdict(&mut shell), expected, "{keys}");
        assert_eq!(shell.tabs(), (3, 2), "{keys}: the active review stays");
        assert!(painted(shell.cx, "submit-dialog"));
    }
    // Closed, ⌘1 is the first open review again.
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    shell.cx.simulate_keystrokes("cmd-1");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (3, 1));
}

/// ADR-0031 (T7.6): the dialog's gaps are on the scale: 12 between a
/// verdict's radio and its label and inside the drafts box, a 6 pt dot.
#[gpui_kit::test]
fn submit_dialog_has_no_off_scale_values(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    open(&mut shell, &tab);
    let bounds = |shell: &mut Shell, name: &str| {
        shell
            .cx
            .debug_bounds(Box::leak(name.to_owned().into_boxed_str()))
            .unwrap_or_else(|| panic!("{name} was not painted"))
    };
    let twelve = gpui_kit::px(12.);
    for slug in ["comment", "approve", "request-changes"] {
        let radio = bounds(&mut shell, &format!("submit-verdict-radio-{slug}"));
        let label = bounds(&mut shell, &format!("submit-verdict-label-{slug}"));
        assert_eq!(label.left() - radio.right(), twelve, "{slug}");
    }
    let status = bounds(&mut shell, "submit-status");
    let drafts = bounds(&mut shell, "submit-drafts");
    assert_eq!(
        drafts.left() - status.left(),
        twelve,
        "the drafts box's inset"
    );
    let dot = bounds(&mut shell, "submit-waiter-dot");
    assert_eq!(dot.size, gpui_kit::size(gpui_kit::px(6.), gpui_kit::px(6.)));
}
