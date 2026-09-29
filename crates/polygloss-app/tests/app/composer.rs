//! GPUI tests of T3.10: the composer and drafts (design §8.2, §8.3, §8.7,
//! OQ-31, OQ-P16): `c` opens a composer under the cursor's line, `⌘⏎`
//! saves a draft (pinning a live diff first), `Esc` cancels, unsaved text
//! is autosaved and restored, file- and review-level composers, replies,
//! edit and delete of one's own comments, resolve at once, and threads of
//! another review offering "Reply in <review>" only.

use std::path::Path;
use std::time::Duration;

use gpui_kit::{Entity, VisualTestContext};
use polygloss_app::composer::{self, ComposerKey};
use polygloss_app::keymap::actions::tab as tab_actions;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads;
use polygloss_app::view_state::SAVE_DEBOUNCE;
use polygloss_core::git::{Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, NewThread, OpenRequest, Subject, ThreadFilter, ThreadKind, ThreadScope,
    ThreadStatus, ThreadView, Viewer,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{BlockAnchor, CursorPos};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
    }
}

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

fn painted(cx: &mut VisualTestContext, selector: String) -> bool {
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
        .is_some()
}

fn click(shell: &mut Shell, selector: String) {
    let bounds = shell
        .cx
        .debug_bounds(Box::leak(selector.clone().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} was not painted"));
    shell
        .cx
        .simulate_click(bounds.center(), gpui_kit::Modifiers::none());
    draw(shell.cx);
}

fn keys(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

fn typed(shell: &mut Shell, text: &str) {
    shell.cx.simulate_input(text);
    draw(shell.cx);
}

/// Every thread of the review as the human sees them (drafts included).
fn threads_of(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<ThreadView> {
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    shell
        .core
        .threads(
            ThreadScope::Review(review),
            Viewer::Human,
            &ThreadFilter::default(),
        )
        .expect("list threads")
}

/// Puts the viewport's cursor on 0-based `line` of `side` in file `file_idx`.
fn cursor_at(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, side: Side, line: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.set_cursor(
            Some(CursorPos {
                file_idx,
                side,
                line,
                range_start: None,
            }),
            cx,
        )
    });
    // The keyboard in the diff.
    let focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    shell.cx.update(|window, cx| window.focus(&focus, cx));
    draw(shell.cx);
}

fn open_keys(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<ComposerKey> {
    tab.read_with(shell.cx, |t, cx| {
        composer::composers(t)
            .map(|c| c.read(cx).keys())
            .unwrap_or_default()
    })
}

fn composer_text(shell: &mut Shell, tab: &Entity<ReviewTab>, key: &ComposerKey) -> Option<String> {
    tab.read_with(shell.cx, |t, cx| {
        composer::composer(t, key, cx).map(|c| c.read(cx).text(cx))
    })
}

fn composer_focused(shell: &mut Shell, tab: &Entity<ReviewTab>, key: &ComposerKey) -> bool {
    let view = tab
        .read_with(shell.cx, |t, cx| composer::composer(t, key, cx))
        .expect("the composer is open");
    shell
        .cx
        .update(|window, cx| view.read(cx).contains_focus(window, cx))
}

fn create(
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
    subject: Subject,
    kind: ThreadKind,
    body: &str,
    author: Author,
) -> String {
    let (review_id, diff_id, repo) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo).expect("open the object store");
    shell
        .core
        .create_thread(
            &NewThread {
                review_id,
                diff_id,
                subject,
                kind,
                body_md: body.into(),
                author,
            },
            &blobs,
        )
        .expect("create the thread")
}

fn reload(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
}

/// `config.rs` new line 5 (0-based 4), as a composer key.
fn line5() -> ComposerKey {
    ComposerKey::Line {
        path: "src/config.rs".into(),
        side: Side::New,
        start_line: 5,
        line: 5,
    }
}

#[gpui_kit::test]
fn composer_keys_round_trip_and_name_their_subjects(_cx: &mut gpui_kit::TestAppContext) {
    let keys = [
        ComposerKey::line("dir/a:b.rs", Side::Old, 9, 7),
        ComposerKey::File {
            path: "x/y z.md".into(),
        },
        ComposerKey::Review,
        ComposerKey::Reply {
            thread_id: "t1".into(),
        },
        ComposerKey::Edit {
            comment_id: "c1".into(),
        },
    ];
    for key in &keys {
        assert_eq!(ComposerKey::parse(&key.to_string()).as_ref(), Some(key));
    }
    // 0-based viewport lines, in either order, become a 1-based range.
    assert_eq!(keys[0].to_string(), "line:old:8:10:dir/a:b.rs");
    assert_eq!(
        keys[0].subject(),
        Some(Subject::Line {
            path: "dir/a:b.rs".into(),
            side: Side::Old,
            start_line: 8,
            line: 10
        })
    );
    assert_eq!(keys[0].title(), "Comment on lines L8–L10");
    assert_eq!(line5().title(), "Comment on line 5");
    assert_eq!(keys[2].subject(), Some(Subject::Review));
    assert_eq!(keys[3].subject(), None);
    for bad in [
        "",
        "line:new:0:1:a",
        "line:new:3:2:a",
        "line:mid:1:1:a",
        "file:",
        "nope:x",
    ] {
        assert_eq!(ComposerKey::parse(bad), None, "{bad:?}");
    }
    // Composer blocks never share an id with thread blocks.
    assert_ne!(
        ComposerKey::Reply {
            thread_id: "x".into()
        }
        .block_id(),
        threads::placement::block_id("x")
    );
}

#[gpui_kit::test]
fn c_opens_composer_on_cursor_line(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    cursor_at(&mut shell, &tab, 0, Side::New, 4);
    keys(&mut shell, "c");
    assert_eq!(open_keys(&mut shell, &tab), [line5()]);
    // Below the cursor's line, in the diff, with the keyboard.
    let blocks = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().blocks(0).to_vec()
    });
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].id, line5().block_id());
    assert_eq!(
        blocks[0].anchor,
        BlockAnchor::Line {
            side: Side::New,
            line: 4
        }
    );
    assert!(painted(shell.cx, format!("composer-{}", line5())));
    assert!(composer_focused(&mut shell, &tab, &line5()));
    // Typing goes to the composer, not to the diff's single-key bindings.
    typed(&mut shell, "jk c");
    assert_eq!(
        composer_text(&mut shell, &tab, &line5()).as_deref(),
        Some("jk c")
    );
    assert_eq!(open_keys(&mut shell, &tab).len(), 1);

    // A range: shift-↓ extends it, `c` comments on both lines.
    cursor_at(&mut shell, &tab, 0, Side::New, 1);
    keys(&mut shell, "shift-down c");
    let range = ComposerKey::Line {
        path: "src/config.rs".into(),
        side: Side::New,
        start_line: 2,
        line: 3,
    };
    assert!(open_keys(&mut shell, &tab).contains(&range));
    // `c` on a line with an open composer focuses it.
    cursor_at(&mut shell, &tab, 0, Side::New, 4);
    keys(&mut shell, "c");
    assert!(composer_focused(&mut shell, &tab, &line5()));
    assert_eq!(open_keys(&mut shell, &tab).len(), 2);
}

#[gpui_kit::test]
fn cmd_enter_saves_draft_with_draft_badge(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    cursor_at(&mut shell, &tab, 0, Side::New, 4);
    keys(&mut shell, "c");
    typed(&mut shell, "Why a `BTreeMap`?");
    keys(&mut shell, "cmd-enter");
    let threads = threads_of(&mut shell, &tab);
    assert_eq!(threads.len(), 1);
    let t = &threads[0];
    assert!(t.draft, "a human's new thread is a draft");
    assert_eq!(
        t.anchor.subject,
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 5,
            line: 5
        }
    );
    // Saved as typed: ⌘⏎ added no newline.
    assert_eq!(t.comments[0].body_md, "Why a `BTreeMap`?");
    // The composer made way for the thread, which shows its Draft badge.
    assert!(open_keys(&mut shell, &tab).is_empty());
    let comment = t.comments[0].id.clone();
    assert!(painted(shell.cx, format!("thread-draft-{comment}")));
    let blocks = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().blocks(0).to_vec()
    });
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].id, threads::placement::block_id(&t.id));
    // The diff has the keyboard back: `j` moves the cursor.
    keys(&mut shell, "j");
    let cursor = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor());
    assert_eq!(cursor.map(|c| c.line), Some(5));

    // A reply from the card is a draft too.
    click(&mut shell, format!("thread-reply-{}", t.id));
    let reply = ComposerKey::Reply {
        thread_id: t.id.clone(),
    };
    assert!(composer_focused(&mut shell, &tab, &reply));
    typed(&mut shell, "Sorted output.");
    keys(&mut shell, "cmd-enter");
    let t = &threads_of(&mut shell, &tab)[0];
    assert_eq!(t.comments.len(), 2);
    assert!(t.comments[1].draft);
    assert_eq!(t.comments[1].body_md, "Sorted output.");
    // Empty text does not save.
    click(&mut shell, format!("thread-reply-{}", t.id));
    keys(&mut shell, "cmd-enter");
    assert!(open_keys(&mut shell, &tab).contains(&reply));
    assert_eq!(threads_of(&mut shell, &tab)[0].comments.len(), 2);
}

#[gpui_kit::test]
fn escape_cancels_without_saving(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    cursor_at(&mut shell, &tab, 0, Side::New, 4);
    keys(&mut shell, "c");
    typed(&mut shell, "never mind");
    keys(&mut shell, "escape");
    assert!(open_keys(&mut shell, &tab).is_empty());
    assert!(threads_of(&mut shell, &tab).is_empty());
    let blocks = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().blocks(0).len()
    });
    assert_eq!(blocks, 0);
    // Its text is not kept: `c` again starts empty.
    shell
        .cx
        .executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    keys(&mut shell, "c");
    assert_eq!(
        composer_text(&mut shell, &tab, &line5()).as_deref(),
        Some("")
    );
    // The Cancel button does the same.
    click(&mut shell, format!("composer-cancel-{}", line5()));
    assert!(open_keys(&mut shell, &tab).is_empty());
}

#[gpui_kit::test]
fn composer_text_autosaved_and_restored(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    cursor_at(&mut shell, &tab, 0, Side::New, 4);
    keys(&mut shell, "c");
    typed(&mut shell, "half a thought");
    shell
        .cx
        .executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
    let stored = shell.core.load_view_state(&diff_id).unwrap().unwrap();
    assert_eq!(
        stored
            .composer
            .get(&line5().to_string())
            .map(String::as_str),
        Some("half a thought")
    );
    // Close the tab and open the review again: the composer is back with
    // its text, without taking the keyboard.
    keys(&mut shell, "escape");
    // (Esc dropped it; type it again and close the tab with it open.)
    keys(&mut shell, "c");
    typed(&mut shell, "half a thought");
    let focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    shell.cx.update(|window, cx| window.focus(&focus, cx));
    keys(&mut shell, "cmd-w");
    shell
        .cx
        .update(|_, cx| cx.set_global(polygloss_app::view_state::SessionViewStates::default()));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    assert_eq!(open_keys(&mut shell, &tab), [line5()]);
    assert_eq!(
        composer_text(&mut shell, &tab, &line5()).as_deref(),
        Some("half a thought")
    );
    assert!(painted(shell.cx, format!("composer-{}", line5())));
    // Saving drops the autosaved text.
    let view = tab
        .read_with(shell.cx, |t, cx| composer::composer(t, &line5(), cx))
        .unwrap();
    shell.cx.update(|window, cx| {
        let handle = view.read(cx).focus_target(cx);
        window.focus(&handle, cx)
    });
    keys(&mut shell, "cmd-enter");
    shell
        .cx
        .executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
    let stored = shell.core.load_view_state(&diff_id).unwrap().unwrap();
    assert!(stored.composer.is_empty());
    assert_eq!(threads_of(&mut shell, &tab).len(), 1);
}

/// A repo on branch `feature` with an uncommitted edit of `src/a.rs`.
fn live_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let base: String = (0..20).map(|i| format!("fn a_{i}() {{}}\n")).collect();
    repo.write("src/a.rs", base.as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    let edited = base.replace("fn a_5() {}", "fn a_5() { edited(); }");
    repo.write("src/a.rs", edited.as_bytes());
    repo
}

fn live_req(worktree: &Path) -> OpenRequest {
    OpenRequest {
        worktree: worktree.to_path_buf(),
        source: Source::Live { since: Since::Head },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

#[gpui_kit::test]
fn comment_on_live_diff_pins_snapshot_first(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = shell.open(live_req(repo.path())).unwrap();
    let (review_id, diff_id, pinned) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.iteration.is_some(),
        )
    });
    assert!(!pinned, "a live open is not pinned");
    assert!(shell.core.iterations(&review_id).unwrap().is_empty());
    // A composer left open with text: the view state to keep once pinned.
    cursor_at(&mut shell, &tab, 0, Side::New, 1);
    keys(&mut shell, "c");
    typed(&mut shell, "later");
    cursor_at(&mut shell, &tab, 0, Side::New, 5);
    keys(&mut shell, "c");
    typed(&mut shell, "Is this edit intended?");
    keys(&mut shell, "cmd-enter");
    // The state shown was pinned as an iteration first (by a comment), and
    // the thread is on that iteration's diff.
    let its = shell.core.iterations(&review_id).unwrap();
    assert_eq!(its.len(), 1);
    assert_eq!(its[0].diff_id, diff_id);
    let by: String = shell
        .core
        .store
        .read(|c| Ok(c.query_row("SELECT pinned_by FROM iterations", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(by, "comment");
    let threads = threads_of(&mut shell, &tab);
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].origin_diff_id, diff_id);
    assert!(threads[0].draft);
    // The tab knows it is pinned now (Snapshot is done).
    let iteration = tab.read_with(shell.cx, |t, _| t.opened.iteration.clone());
    assert_eq!(iteration.map(|i| i.seq), Some(1));
    // The session's view state reached the store once pinned (T3.14).
    draw(shell.cx);
    let stored = shell.core.load_view_state(&diff_id).unwrap().unwrap();
    assert_eq!(
        stored
            .composer
            .get("line:new:2:2:src/a.rs")
            .map(String::as_str),
        Some("later")
    );
}

#[gpui_kit::test]
fn resolve_is_immediate_not_draft(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let id = create(
        &mut shell,
        &tab,
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 5,
            line: 5,
        },
        ThreadKind::Question,
        "Should this be sorted?",
        agent(),
    );
    reload(&mut shell, &tab);
    click(&mut shell, format!("thread-resolve-{id}"));
    // Stored at once, by the human, with no draft to submit.
    let t = shell.core.thread(&id, Viewer::Agent).unwrap();
    assert_eq!(t.status, ThreadStatus::Resolved);
    assert_eq!(t.resolved_by.map(|r| r.kind), Some(AuthorKind::Human));
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    assert_eq!(shell.core.drafts_count(&review).unwrap(), 0);
    // The block collapsed to its chip; opening it offers Unresolve.
    assert!(painted(shell.cx, format!("thread-chip-{id}")));
    click(&mut shell, format!("thread-chip-{id}"));
    click(&mut shell, format!("thread-resolve-{id}"));
    assert_eq!(
        shell.core.thread(&id, Viewer::Agent).unwrap().status,
        ThreadStatus::Open
    );
    // A human draft thread has nothing to resolve yet.
    let draft = create(
        &mut shell,
        &tab,
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 2,
            line: 2,
        },
        ThreadKind::Comment,
        "draft",
        human(),
    );
    reload(&mut shell, &tab);
    assert!(painted(shell.cx, format!("thread-reply-{draft}")));
    assert!(!painted(shell.cx, format!("thread-resolve-{draft}")));
}

#[gpui_kit::test]
fn edit_and_delete_own_only(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let theirs = create(
        &mut shell,
        &tab,
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 2,
            line: 2,
        },
        ThreadKind::Comment,
        "Agent says hi",
        agent(),
    );
    let mine = create(
        &mut shell,
        &tab,
        Subject::Line {
            path: "src/config.rs".into(),
            side: Side::New,
            start_line: 5,
            line: 5,
        },
        ThreadKind::Comment,
        "First try",
        human(),
    );
    reload(&mut shell, &tab);
    let comment_of = |shell: &mut Shell, id: &str| {
        shell.core.thread(id, Viewer::Human).unwrap().comments[0]
            .id
            .clone()
    };
    let (their_c, my_c) = (
        comment_of(&mut shell, &theirs),
        comment_of(&mut shell, &mine),
    );
    assert!(painted(shell.cx, format!("comment-edit-{my_c}")));
    assert!(painted(shell.cx, format!("comment-delete-{my_c}")));
    assert!(!painted(shell.cx, format!("comment-edit-{their_c}")));
    assert!(!painted(shell.cx, format!("comment-delete-{their_c}")));
    // Agents' comments cannot be edited from here either.
    let refused = shell
        .cx
        .update(|window, cx| tab.update(cx, |t, cx| composer::open_edit(t, &their_c, window, cx)));
    assert!(!refused);

    // Edit: the composer holds the body; ⌘⏎ updates it.
    click(&mut shell, format!("comment-edit-{my_c}"));
    let edit = ComposerKey::Edit {
        comment_id: my_c.clone(),
    };
    assert_eq!(
        composer_text(&mut shell, &tab, &edit).as_deref(),
        Some("First try")
    );
    assert!(composer_focused(&mut shell, &tab, &edit));
    typed(&mut shell, ", edited");
    keys(&mut shell, "cmd-enter");
    let t = shell.core.thread(&mine, Viewer::Human).unwrap();
    assert_eq!(t.comments[0].body_md, "First try, edited");
    assert!(open_keys(&mut shell, &tab).is_empty());

    // Delete: a draft goes at once, and its thread with it.
    click(&mut shell, format!("comment-delete-{my_c}"));
    assert!(shell.core.thread(&mine, Viewer::Human).is_err());
    assert_eq!(threads_of(&mut shell, &tab).len(), 1);
    assert!(!painted(shell.cx, format!("thread-{mine}")));
}

#[gpui_kit::test]
fn comment_on_file_creates_file_thread_draft(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // The cursor's file (src/greet.ts); the palette action.
    cursor_at(&mut shell, &tab, 1, Side::New, 2);
    shell.cx.dispatch_action(tab_actions::CommentOnFile);
    draw(shell.cx);
    let key = ComposerKey::File {
        path: "src/greet.ts".into(),
    };
    assert_eq!(open_keys(&mut shell, &tab), std::slice::from_ref(&key));
    let blocks = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().blocks(1).to_vec()
    });
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].anchor, BlockAnchor::FileTop);
    assert!(composer_focused(&mut shell, &tab, &key));
    typed(&mut shell, "Split this file?");
    keys(&mut shell, "cmd-enter");
    let threads = threads_of(&mut shell, &tab);
    assert_eq!(threads.len(), 1);
    assert!(threads[0].draft);
    assert_eq!(
        threads[0].anchor.subject,
        Subject::File {
            path: "src/greet.ts".into()
        }
    );
    // The header's ⋯ "Comment on file" (FileCommentRequested) opens one too.
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |_, cx| {
        cx.emit(polygloss_viewport::ViewportEvent::FileCommentRequested(2))
    });
    draw(shell.cx);
    assert_eq!(
        open_keys(&mut shell, &tab),
        [ComposerKey::File {
            path: "src/main.rs".into()
        }]
    );
}

#[gpui_kit::test]
fn comment_on_review_creates_review_thread_draft(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // Hidden panel: the action shows it.
    tab.update(shell.cx, |t, cx| t.toggle_threads_panel(cx));
    draw(shell.cx);
    cursor_at(&mut shell, &tab, 0, Side::New, 1);
    shell.cx.dispatch_action(tab_actions::CommentOnReview);
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    assert_eq!(open_keys(&mut shell, &tab), [ComposerKey::Review]);
    assert!(painted(shell.cx, "composer-review".into()));
    assert!(composer_focused(&mut shell, &tab, &ComposerKey::Review));
    typed(&mut shell, "Overall this looks right.");
    keys(&mut shell, "cmd-enter");
    let threads = threads_of(&mut shell, &tab);
    assert_eq!(threads.len(), 1);
    assert!(threads[0].draft);
    assert_eq!(threads[0].anchor.subject, Subject::Review);
    assert!(painted(
        shell.cx,
        format!("threads-panel-{}", threads[0].id)
    ));
    // The panel's button opens it too.
    click(&mut shell, "comment-on-review".into());
    assert_eq!(open_keys(&mut shell, &tab), [ComposerKey::Review]);
}

#[gpui_kit::test]
fn other_reviews_thread_offers_reply_in_its_review_only(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // Review S: the head commit (its diff is base → head, the same trees as
    // the compare review below), with an agent's published thread.
    let commit = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit {
            rev: "refs/tags/head".into(),
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let s = shell.core.open(&commit).unwrap();
    let blobs = BlobReader::open(&s.repo).unwrap();
    let theirs = shell
        .core
        .create_thread(
            &NewThread {
                review_id: s.review_id.clone(),
                diff_id: s.diff_id.clone(),
                subject: Subject::Line {
                    path: "src/config.rs".into(),
                    side: Side::New,
                    start_line: 5,
                    line: 5,
                },
                kind: ThreadKind::Question,
                body_md: "Asked in the commit review".into(),
                author: agent(),
            },
            &blobs,
        )
        .unwrap();
    // Review R shows it through its origin diff.
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let r = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    assert_ne!(r, s.review_id);
    assert_eq!(
        tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone()),
        s.diff_id
    );
    draw(shell.cx);
    assert!(painted(shell.cx, format!("thread-{theirs}")));
    // No reply box: "Reply in <that review>" (named by its commit's
    // subject), and Resolve.
    assert!(!painted(shell.cx, format!("thread-reply-{theirs}")));
    assert!(painted(
        shell.cx,
        format!("thread-reply-elsewhere-{theirs}")
    ));
    assert!(painted(shell.cx, format!("thread-resolve-{theirs}")));
    let other = tab.read_with(shell.cx, |t, cx| {
        composer::composers(t).and_then(|c| c.read(cx).other_review(&s.review_id).cloned())
    });
    match other {
        Some(composer::OtherReview::Known { title, open }) => {
            assert_eq!(title, "head");
            assert!(open.is_some());
        }
        other => panic!("the other review was not looked up: {other:?}"),
    }
    let refused = shell
        .cx
        .update(|window, cx| tab.update(cx, |t, cx| composer::open_reply(t, &theirs, window, cx)));
    assert!(!refused, "no reply composer on another review's thread");
    assert!(open_keys(&mut shell, &tab).is_empty());
    // Resolving stays, without a closing reply.
    click(&mut shell, format!("thread-resolve-{theirs}"));
    let t = shell.core.thread(&theirs, Viewer::Agent).unwrap();
    assert_eq!(t.status, ThreadStatus::Resolved);
    assert_eq!(t.comments.len(), 1);
    assert_eq!(shell.core.drafts_count(&r).unwrap(), 0);
    assert_eq!(shell.core.drafts_count(&s.review_id).unwrap(), 0);
    // "Reply in" opens S's tab.
    click(&mut shell, format!("thread-chip-{theirs}"));
    let (tabs_before, _) = shell.tabs();
    click(&mut shell, format!("thread-reply-elsewhere-{theirs}"));
    let (tabs_after, _) = shell.tabs();
    assert_eq!(tabs_after, tabs_before + 1);
    let active = shell.active_review().expect("a review tab is active");
    assert_eq!(
        active.read_with(shell.cx, |t, _| t.review_id.clone()),
        s.review_id
    );
    // There, the thread is its own: a reply box (in its card, opened).
    draw(shell.cx);
    click(&mut shell, format!("thread-chip-{theirs}"));
    assert!(painted(shell.cx, format!("thread-reply-{theirs}")));
    assert!(!painted(
        shell.cx,
        format!("thread-reply-elsewhere-{theirs}")
    ));
}

#[gpui_kit::test]
fn restored_reply_on_another_reviews_thread_is_dropped(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // Review S (the head commit) and review R (base..head) share one diff,
    // and with it one view state: a reply S's tab autosaved there must not
    // reopen in R's tab, where the thread is not R's (OQ-P16).
    let commit = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit {
            rev: "refs/tags/head".into(),
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let s = shell.core.open(&commit).unwrap();
    let blobs = BlobReader::open(&s.repo).unwrap();
    let theirs = shell
        .core
        .create_thread(
            &NewThread {
                review_id: s.review_id.clone(),
                diff_id: s.diff_id.clone(),
                subject: Subject::Line {
                    path: "src/config.rs".into(),
                    side: Side::New,
                    start_line: 5,
                    line: 5,
                },
                kind: ThreadKind::Question,
                body_md: "Asked in the commit review".into(),
                author: agent(),
            },
            &blobs,
        )
        .unwrap();
    let mut state = shell
        .core
        .load_view_state(&s.diff_id)
        .unwrap()
        .unwrap_or_default();
    state
        .composer
        .insert(format!("reply:{theirs}"), "half a reply".into());
    shell.core.save_view_state(&s.diff_id, &state).unwrap();
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    draw(shell.cx);
    assert_eq!(
        tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone()),
        s.diff_id
    );
    let shown = tab.read_with(shell.cx, |t, cx| {
        threads::threads(t).is_some_and(|m| m.read(cx).thread(&theirs).is_some())
    });
    assert!(shown, "R's tab shows S's thread");
    assert!(
        open_keys(&mut shell, &tab).is_empty(),
        "no reply composer on another review's thread"
    );
    // Dropping it here must not delete S's draft text: the view state is
    // shared, and S's tab restores it later.
    shell
        .cx
        .executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
    let kept = |shell: &mut Shell| {
        shell
            .core
            .load_view_state(&s.diff_id)
            .unwrap()
            .and_then(|v| v.composer.get(&format!("reply:{theirs}")).cloned())
    };
    assert_eq!(kept(&mut shell).as_deref(), Some("half a reply"));
    // R saving its own view state afterwards keeps it too.
    cursor_at(&mut shell, &tab, 0, Side::New, 4);
    keys(&mut shell, "c");
    typed(&mut shell, "R's own thought");
    shell
        .cx
        .executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
    let stored = shell.core.load_view_state(&s.diff_id).unwrap().unwrap();
    assert_eq!(
        stored
            .composer
            .get(&line5().to_string())
            .map(String::as_str),
        Some("R's own thought")
    );
    assert_eq!(kept(&mut shell).as_deref(), Some("half a reply"));
}
