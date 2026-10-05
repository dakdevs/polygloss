//! GPUI tests of T3.12: iterations and "Changes since last review" (design
//! §11.4, §8.6, OQ-9). The toolbar's "Iteration k of n" picker switches the
//! diff the tab shows (threads carried forward), and "Changes since last
//! review" shows the diff from the head of the iteration the last
//! submission was made against to the current head, in the same tab, with
//! comments on the new side only.

use gpui_kit::Entity;
use polygloss_app::composer::{self, ComposerKey};
use polygloss_app::iterations::{self, Choice, Showing};
use polygloss_app::live;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads::{self, placement::ThreadPlace};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, NewThread, OpenRequest, PositionState, Subject, ThreadKind, Verdict,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{CursorPos, ScrollAnchor, ViewportEvent};

use crate::shell::{Shell, draw, start};
use crate::support::{FixtureRepo, Sandbox};

fn numbered(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("fn {prefix}_{i}() {{}}\n"))
        .collect()
}

/// `main`: `src/a.rs`, `src/b.rs` and `src/c.rs`, 120 lines each. Branch
/// `feature` (checked out), commit 1: `b.rs` line 51 and `c.rs` line 11
/// changed. Review `main...feature` shows `b.rs` and `c.rs`.
fn review_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/a.rs", numbered("a", 120).as_bytes());
    repo.write("src/b.rs", numbered("b", 120).as_bytes());
    repo.write("src/c.rs", numbered("c", 120).as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write(
        "src/b.rs",
        numbered("b", 120)
            .replace("fn b_50() {}", "fn b_50() { changed(); }")
            .as_bytes(),
    );
    repo.write(
        "src/c.rs",
        numbered("c", 120)
            .replace("fn c_10() {}", "fn c_10() { changed(); }")
            .as_bytes(),
    );
    repo.commit("feature 1");
    repo
}

/// Commit 2 on `feature`: `a.rs` line 11 changed and five lines added at
/// the top of `b.rs` (its other lines move down by five); `c.rs` untouched.
fn second_commit(repo: &FixtureRepo) {
    repo.write(
        "src/a.rs",
        numbered("a", 120)
            .replace("fn a_10() {}", "fn a_10() { changed(); }")
            .as_bytes(),
    );
    let top: String = (0..5).map(|i| format!("// header {i}\n")).collect();
    let b = numbered("b", 120).replace("fn b_50() {}", "fn b_50() { changed(); }");
    repo.write("src/b.rs", (top + &b).as_bytes());
    repo.commit("feature 2");
}

fn compare_req(repo: &FixtureRepo) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::ThreeDot,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// The tab's refresh (`R`): the compare review records its next iteration.
fn refresh(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    tab.update_in(shell.cx, live::refresh_tab);
    draw(shell.cx);
}

fn show(shell: &mut Shell, tab: &Entity<ReviewTab>, choice: Choice) {
    tab.update_in(shell.cx, |t, window, cx| {
        iterations::show(t, choice, window, cx)
    });
    draw(shell.cx);
}

fn toggle_since(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    tab.update_in(shell.cx, |t, window, cx| {
        iterations::toggle_changes_since(t, window, cx)
    });
    draw(shell.cx);
}

fn showing(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Showing {
    tab.read_with(shell.cx, |t, _| iterations::showing(t))
}

fn label(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, _| iterations::label(t))
}

fn paths(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<String> {
    tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect()
    })
}

/// The files the viewport shows.
fn viewport_paths(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<String> {
    tab.read_with(shell.cx, |t, cx| {
        t.viewport
            .read(cx)
            .document()
            .files()
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect()
    })
}

fn diff_id(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, _| t.opened.diff_id.as_str().to_owned())
}

fn submit(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    let review_id = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    shell
        .core
        .submit_review(&review_id, Verdict::RequestChanges, "Please fix", None)
        .expect("submit");
    tab.update(shell.cx, iterations::reload);
    draw(shell.cx);
}

#[gpui_kit::test]
fn iteration_picker_lists_and_switches(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    let first = diff_id(&mut shell, &tab);
    // One iteration and no submission: nothing to pick yet.
    assert_eq!(label(&mut shell, &tab), "Iteration 1 of 1");
    assert!(!tab.read_with(shell.cx, |t, _| iterations::picker_visible(t)));

    second_commit(&repo);
    refresh(&mut shell, &tab);
    let second = diff_id(&mut shell, &tab);
    assert_ne!(first, second);
    assert_eq!(showing(&mut shell, &tab), Showing::Current);
    assert_eq!(label(&mut shell, &tab), "Iteration 2 of 2");
    assert!(tab.read_with(shell.cx, |t, _| iterations::picker_visible(t)));
    let entries = tab.read_with(shell.cx, |t, _| iterations::picker_entries(t));
    let listed: Vec<(Choice, &str, bool)> = entries
        .iter()
        .map(|e| (e.choice, e.label.as_str(), e.selected))
        .collect();
    assert_eq!(
        listed,
        [
            (Choice::Current, "Iteration 2", true),
            (Choice::Iteration(1), "Iteration 1", false),
        ]
    );
    assert!(entries[0].detail.contains("latest"), "{:?}", entries[0]);

    // Iteration 1: its diff, in the same tab and viewport.
    show(&mut shell, &tab, Choice::Iteration(1));
    assert_eq!(showing(&mut shell, &tab), Showing::Iteration(1));
    assert_eq!(diff_id(&mut shell, &tab), first);
    assert_eq!(paths(&mut shell, &tab), ["src/b.rs", "src/c.rs"]);
    assert_eq!(viewport_paths(&mut shell, &tab), ["src/b.rs", "src/c.rs"]);
    assert_eq!(label(&mut shell, &tab), "Iteration 1 of 2");
    let context = tab.read_with(shell.cx, |t, cx| t.banners.read(cx).context().to_string());
    assert!(
        context.starts_with("Iteration 1 of 2"),
        "banner line: {context}"
    );
    let entries = tab.read_with(shell.cx, |t, _| iterations::picker_entries(t));
    assert!(entries[1].selected && !entries[0].selected);
    // The tree follows.
    let tree_files = tab.read_with(shell.cx, |t, cx| {
        polygloss_app::tree::file_tree(t)
            .expect("a tree")
            .read(cx)
            .model()
            .file_order()
            .len()
    });
    assert_eq!(tree_files, 2);

    // Back to the latest.
    show(&mut shell, &tab, Choice::Current);
    assert_eq!(showing(&mut shell, &tab), Showing::Current);
    assert_eq!(diff_id(&mut shell, &tab), second);
    assert_eq!(
        viewport_paths(&mut shell, &tab),
        ["src/a.rs", "src/b.rs", "src/c.rs"]
    );
    assert_eq!(label(&mut shell, &tab), "Iteration 2 of 2");
}

#[gpui_kit::test]
fn changes_since_last_review_diffs_submission_head_to_current(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let submitted_head = repo.oid("feature^{tree}");
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    submit(&mut shell, &tab);
    second_commit(&repo);
    refresh(&mut shell, &tab);
    let full = diff_id(&mut shell, &tab);
    assert!(tab.read_with(shell.cx, |t, _| iterations::changes_since_available(t)));

    toggle_since(&mut shell, &tab);
    assert_eq!(
        showing(&mut shell, &tab),
        Showing::ChangesSince { since: 1 }
    );
    // Only what changed after the review: `c.rs` did not.
    assert_eq!(paths(&mut shell, &tab), ["src/a.rs", "src/b.rs"]);
    assert_eq!(viewport_paths(&mut shell, &tab), ["src/a.rs", "src/b.rs"]);
    let (base, head, stored) = tab.read_with(shell.cx, |t, _| {
        (
            t.opened.base.tree.clone(),
            t.opened.head_tree.clone(),
            t.opened.diff_id.clone(),
        )
    });
    assert_eq!(base, submitted_head);
    assert_eq!(head, repo.oid("feature^{tree}"));
    assert!(
        shell.core.files_for_diff(&stored).unwrap().is_some(),
        "a pinned diff"
    );
    assert_eq!(label(&mut shell, &tab), "Changes since last review");
    let context = tab.read_with(shell.cx, |t, cx| t.banners.read(cx).context().to_string());
    assert!(
        context.starts_with("Changes since your last review"),
        "banner line: {context}"
    );
    // The toggle is checked in the picker's menu.
    let entries = tab.read_with(shell.cx, |t, _| iterations::picker_entries(t));
    assert!(entries.iter().all(|e| !e.selected));
    assert!(tab.read_with(shell.cx, |t, _| iterations::changes_since_checked(t)));

    // The palette action toggles it off again: the full diff is back.
    let focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    shell.cx.update(|window, cx| {
        focus.dispatch_action(
            &polygloss_app::keymap::actions::tab::ToggleChangesSinceLastReview,
            window,
            cx,
        );
    });
    draw(shell.cx);
    assert_eq!(showing(&mut shell, &tab), Showing::Current);
    assert_eq!(diff_id(&mut shell, &tab), full);
    assert_eq!(paths(&mut shell, &tab).len(), 3);
}

#[gpui_kit::test]
fn changes_since_on_a_live_review_pins_the_working_tree_first(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    repo.write("src/a.rs", b"fn edited() {}\n");
    let mut shell = start(cx);
    let tab = shell
        .open(OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Live {
                since: Since::MergeBase,
            },
            label: None,
            pin: None,
            actor: Actor::human(),
        })
        .expect("open the live review");
    let (review_id, base, state) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.base.clone(),
            t.opened.live.clone().unwrap(),
        )
    });
    shell
        .core
        .submit_review(&review_id, Verdict::Comment, "", Some((&base, &state)))
        .expect("submit");
    // The agent edits `c.rs`; the tab refreshes to the unpinned state.
    repo.write("src/c.rs", b"fn rewritten() {}\n");
    tab.update_in(shell.cx, live::refresh_tab);
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, _| t.opened.iteration.is_none()));
    assert_eq!(label(&mut shell, &tab), "Working tree");

    toggle_since(&mut shell, &tab);
    assert_eq!(
        showing(&mut shell, &tab),
        Showing::ChangesSince { since: 1 }
    );
    assert_eq!(paths(&mut shell, &tab), ["src/c.rs"]);
    // The working tree was pinned as iteration 2 to have a durable head.
    let its = shell.core.iterations(&review_id).unwrap();
    assert_eq!(its.len(), 2);
    let current = tab.read_with(shell.cx, |t, _| iterations::current(t).clone());
    assert_eq!(current.iteration.as_ref(), Some(&its[1]));
    assert_eq!(its[1].diff_id, current.diff_id);
    assert!(tab.read_with(shell.cx, |t, _| t.opened.live.is_none()));
}

#[gpui_kit::test]
fn changes_since_mode_blocks_old_side_comments(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    submit(&mut shell, &tab);
    second_commit(&repo);
    refresh(&mut shell, &tab);
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    assert!(viewport.read_with(shell.cx, |v, _| v.old_side_comments()));

    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = requests.clone();
    let _sub = shell.cx.update(|_, cx| {
        cx.subscribe(&viewport, move |_, event: &ViewportEvent, _| {
            if let ViewportEvent::CommentRequested { side, .. } = event {
                seen.borrow_mut().push(*side);
            }
        })
    });
    let comment_at = |shell: &mut Shell, side: Side, line: u32| {
        viewport.update(shell.cx, |v, cx| {
            v.set_cursor(
                Some(CursorPos {
                    file_idx: 1,
                    side,
                    line,
                    range_start: None,
                }),
                cx,
            );
            v.request_comment(cx);
        });
        draw(shell.cx);
    };

    toggle_since(&mut shell, &tab);
    assert!(!viewport.read_with(shell.cx, |v, _| v.old_side_comments()));
    comment_at(&mut shell, Side::Old, 3);
    assert!(requests.borrow().is_empty(), "no old-side comment");
    comment_at(&mut shell, Side::New, 3);
    assert_eq!(*requests.borrow(), [Side::New]);
    let context = tab.read_with(shell.cx, |t, cx| t.banners.read(cx).context().to_string());
    assert!(context.contains("new lines only"), "banner line: {context}");

    // Leaving the mode allows them again.
    toggle_since(&mut shell, &tab);
    assert!(viewport.read_with(shell.cx, |v, _| v.old_side_comments()));
    comment_at(&mut shell, Side::Old, 3);
    assert_eq!(*requests.borrow(), [Side::New, Side::Old]);
}

#[gpui_kit::test]
fn threads_carry_forward_when_switching_iterations(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    // A thread on `fn b_60() {}` (new side, 1-based line 61) of iteration 1.
    let (review_id, diff, repo_info) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo_info).unwrap();
    let id = shell
        .core
        .create_thread(
            &NewThread {
                review_id,
                diff_id: diff,
                subject: Subject::Line {
                    path: "src/b.rs".into(),
                    side: Side::New,
                    start_line: 61,
                    line: 61,
                },
                kind: ThreadKind::Comment,
                body_md: "Why this name?".into(),
                author: Author {
                    kind: AuthorKind::Human,
                    name: "you".into(),
                    session_id: None,
                },
            },
            &blobs,
        )
        .expect("create the thread");
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);

    let place = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            let m = threads::threads(t).expect("threads").read(cx);
            (m.position(&id).map(|p| p.state), m.place(&id).cloned())
        })
    };
    let at = |file_idx: u32, line: u32| ThreadPlace::Line {
        file_idx,
        side: Side::New,
        start_line: line,
        line,
    };
    assert_eq!(
        place(&mut shell),
        (Some(PositionState::Exact), Some(at(0, 60)))
    );

    // Iteration 2 moves the line down by five (and adds `a.rs` first).
    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert_eq!(
        place(&mut shell),
        (Some(PositionState::Moved), Some(at(1, 65)))
    );

    // Back to iteration 1: exactly where it was written.
    show(&mut shell, &tab, Choice::Iteration(1));
    assert_eq!(
        place(&mut shell),
        (Some(PositionState::Exact), Some(at(0, 60)))
    );
    let blocks = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().blocks(0).len()
    });
    assert_eq!(blocks, 1, "the thread's block is placed again");

    // And to iteration 2 again.
    show(&mut shell, &tab, Choice::Current);
    assert_eq!(
        place(&mut shell),
        (Some(PositionState::Moved), Some(at(1, 65)))
    );
}

/// An open line composer (T3.10) moves with its file when the tab swaps
/// another diff in (a refresh, another iteration), next to the threads
/// placed again (wave-5 integration of T3.10 and T3.12).
#[gpui_kit::test]
fn open_composer_follows_its_file_across_iterations(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    // A thread on `b.rs` too, so its block is placed again on each swap.
    let (review_id, diff, repo_info) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo_info).unwrap();
    shell
        .core
        .create_thread(
            &NewThread {
                review_id,
                diff_id: diff,
                subject: Subject::Line {
                    path: "src/b.rs".into(),
                    side: Side::New,
                    start_line: 61,
                    line: 61,
                },
                kind: ThreadKind::Comment,
                body_md: "Why this name?".into(),
                author: Author {
                    kind: AuthorKind::Human,
                    name: "you".into(),
                    session_id: None,
                },
            },
            &blobs,
        )
        .expect("create the thread");
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
    // A composer on `c.rs` (file 1 of iteration 1) line 30.
    tab.update_in(shell.cx, |t, window, cx| {
        composer::open_line(t, 1, Side::New, 29, 29, window, cx)
    });
    draw(shell.cx);
    let key = ComposerKey::line("src/c.rs", Side::New, 29, 29);
    let block = key.block_id();
    let files_with_it = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            let doc = t.viewport.read(cx).document();
            (0..doc.files().len() as u32)
                .filter(|&f| doc.blocks(f).iter().any(|b| b.id == block))
                .collect::<Vec<_>>()
        })
    };
    let thread_blocks = |shell: &mut Shell, f: u32| {
        tab.read_with(shell.cx, |t, cx| {
            t.viewport
                .read(cx)
                .document()
                .blocks(f)
                .iter()
                .filter(|b| b.id != block)
                .count()
        })
    };
    assert_eq!(files_with_it(&mut shell), vec![1]);

    // Iteration 2 adds `a.rs` first: `c.rs` is file 2 now.
    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert_eq!(
        paths(&mut shell, &tab),
        ["src/a.rs", "src/b.rs", "src/c.rs"]
    );
    assert_eq!(files_with_it(&mut shell), vec![2]);
    assert_eq!(
        thread_blocks(&mut shell, 1),
        1,
        "the thread is placed again"
    );
    let open = tab.read_with(shell.cx, |t, cx| {
        composer::composers(t)
            .map(|c| c.read(cx).keys())
            .unwrap_or_default()
    });
    assert_eq!(open, vec![key.clone()]);

    // Back to iteration 1: file 1 again.
    show(&mut shell, &tab, Choice::Iteration(1));
    assert_eq!(files_with_it(&mut shell), vec![1]);
    assert_eq!(thread_blocks(&mut shell, 0), 1);
}

#[gpui_kit::test]
fn reply_text_survives_its_thread_leaving_with_an_iteration_switch(
    cx: &mut gpui_kit::TestAppContext,
) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    let (review_id, diff, repo_info) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo_info).unwrap();
    let human = Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    };
    let thread_on = |shell: &mut Shell, line: u32| {
        shell
            .core
            .create_thread(
                &NewThread {
                    review_id: review_id.clone(),
                    diff_id: diff.clone(),
                    subject: Subject::Line {
                        path: "src/c.rs".into(),
                        side: Side::New,
                        start_line: line,
                        line,
                    },
                    kind: ThreadKind::Comment,
                    body_md: format!("About line {line}"),
                    author: human.clone(),
                },
                &blobs,
            )
            .expect("create the thread")
    };
    let (kept, dropped) = (thread_on(&mut shell, 11), thread_on(&mut shell, 12));
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
    let root = |shell: &mut Shell, id: &str| {
        shell
            .core
            .thread(id, polygloss_core::review::Viewer::Human)
            .unwrap()
            .comments[0]
            .id
            .clone()
    };
    let (kept_root, dropped_root) = (root(&mut shell, &kept), root(&mut shell, &dropped));
    let reply_to = |shell: &mut Shell, id: &str, text: &str| {
        let opened = tab.update_in(shell.cx, |t, window, cx| {
            composer::open_reply(t, id, window, cx)
        });
        assert!(opened);
        draw(shell.cx);
        shell.cx.simulate_input(text);
        draw(shell.cx);
    };
    let open_keys = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            composer::composers(t)
                .map(|c| c.read(cx).keys())
                .unwrap_or_default()
        })
    };
    let saved = |shell: &mut Shell, id: &str| {
        tab.read_with(shell.cx, |t, _| {
            polygloss_app::view_state::composer_text(t, &format!("reply:{id}")).map(str::to_owned)
        })
    };

    // A thread gone on the diff shown (deleted): its reply goes with it.
    reply_to(&mut shell, &dropped, "never mind");
    assert_eq!(saved(&mut shell, &dropped).as_deref(), Some("never mind"));
    shell.core.delete_comment(&dropped_root, &human).unwrap();
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
    assert!(open_keys(&mut shell).is_empty());
    assert_eq!(saved(&mut shell, &dropped), None);

    // A thread that is not listed any more once the tab shows the next
    // iteration: the reply closes, but its text is kept (close, not cancel).
    reply_to(&mut shell, &kept, "half a reply");
    assert_eq!(
        open_keys(&mut shell),
        [ComposerKey::Reply {
            thread_id: kept.clone()
        }]
    );
    shell.core.delete_comment(&kept_root, &human).unwrap();
    second_commit(&repo);
    refresh(&mut shell, &tab);
    draw(shell.cx);
    assert_ne!(diff_id(&mut shell, &tab), diff.as_str());
    let listed = tab.read_with(shell.cx, |t, cx| {
        threads::threads(t)
            .is_some_and(|m| m.read(cx).is_loaded() && m.read(cx).thread(&kept).is_none())
    });
    assert!(listed, "the threads reloaded without it");
    assert!(open_keys(&mut shell).is_empty(), "the reply closed");
    assert_eq!(saved(&mut shell, &kept).as_deref(), Some("half a reply"));
}

#[gpui_kit::test]
fn no_submission_disables_toggle(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    second_commit(&repo);
    refresh(&mut shell, &tab);
    let before = diff_id(&mut shell, &tab);

    assert!(!tab.read_with(shell.cx, |t, _| iterations::changes_since_available(t)));
    let reason = tab.read_with(shell.cx, |t, _| iterations::changes_since_hint(t));
    assert_eq!(reason, "Submit a review first");
    // The toggle does nothing.
    toggle_since(&mut shell, &tab);
    assert_eq!(showing(&mut shell, &tab), Showing::Current);
    assert_eq!(diff_id(&mut shell, &tab), before);

    // A submission of the head shown: nothing changed since it yet.
    submit(&mut shell, &tab);
    assert!(!tab.read_with(shell.cx, |t, _| iterations::changes_since_available(t)));
    let reason = tab.read_with(shell.cx, |t, _| iterations::changes_since_hint(t));
    assert_eq!(reason, "Nothing changed since your last review");
    // The branch moves on: now there is something to show.
    repo.write("src/c.rs", b"fn c() {}\n");
    repo.commit("feature 3");
    refresh(&mut shell, &tab);
    assert!(tab.read_with(shell.cx, |t, _| iterations::changes_since_available(t)));
    let reason = tab.read_with(shell.cx, |t, _| iterations::changes_since_hint(t));
    assert_eq!(
        reason,
        "Show only what changed since you submitted iteration 2"
    );
}

/// Viewed marks follow the diff shown (design §9): `b.rs`, viewed in
/// iteration 1, changed in iteration 2 ("changed since viewed" there) and
/// is viewed again when iteration 1 is shown.
#[gpui_kit::test]
fn viewed_marks_follow_the_iteration_shown(cx: &mut gpui_kit::TestAppContext) {
    use polygloss_core::review::ViewedState;
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    tab.update(shell.cx, |t, cx| {
        polygloss_app::viewed::set_viewed(t, &[0], true, cx)
    });
    draw(shell.cx);
    let states = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, _| {
            polygloss_app::viewed::states(t)
                .unwrap_or_default()
                .to_vec()
        })
    };
    assert_eq!(
        states(&mut shell),
        [ViewedState::Viewed, ViewedState::NotViewed]
    );

    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert_eq!(
        states(&mut shell),
        [
            ViewedState::NotViewed,
            ViewedState::ChangedSinceViewed,
            ViewedState::NotViewed
        ]
    );
    show(&mut shell, &tab, Choice::Iteration(1));
    assert_eq!(
        states(&mut shell),
        [ViewedState::Viewed, ViewedState::NotViewed]
    );
    let flags = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).file_flags().to_vec());
    assert!(flags[0].viewed && !flags[0].changed_since_viewed);
}

#[gpui_kit::test]
fn refresh_and_iteration_switches_keep_the_top_of_the_document(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    // The header card above the first card, reloaded by every switch.
    let prelude = viewport.read_with(shell.cx, |v, _| v.document().prelude_height());
    assert!(prelude.is_some_and(|h| h > 0.0), "{prelude:?}");
    // The scroll position, and whether it is the top anchor (a short
    // document clamps `scroll_top` to 0 whatever the anchor).
    let top = |shell: &mut Shell| {
        viewport.read_with(shell.cx, |v, _| {
            (
                v.document().scroll_top(),
                v.anchor() == ScrollAnchor::default(),
            )
        })
    };
    assert_eq!(top(&mut shell), (0.0, true));

    // Iteration 2 adds `a.rs`, which sorts before the first file (`b.rs`).
    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert_eq!(
        paths(&mut shell, &tab),
        ["src/a.rs", "src/b.rs", "src/c.rs"]
    );
    assert_eq!(top(&mut shell), (0.0, true));
    // Iteration 1 has no `a.rs`: the first file goes away.
    show(&mut shell, &tab, Choice::Iteration(1));
    assert_eq!(paths(&mut shell, &tab), ["src/b.rs", "src/c.rs"]);
    assert_eq!(top(&mut shell), (0.0, true));
    // And back: a file sorting first appears.
    show(&mut shell, &tab, Choice::Current);
    assert_eq!(
        paths(&mut shell, &tab),
        ["src/a.rs", "src/b.rs", "src/c.rs"]
    );
    assert_eq!(top(&mut shell), (0.0, true));
    assert_eq!(
        viewport.read_with(shell.cx, |v, _| v.document().prelude_height()),
        prelude
    );
    assert_eq!(
        crate::shell::bounds(shell.cx, "header-card").top(),
        crate::shell::bounds(shell.cx, "viewport-pane").top()
    );
}

#[gpui_kit::test]
fn iteration_pill_shows_only_with_more_than_one_state(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    // One state and no submission: no pill.
    assert!(crate::shell::painted(shell.cx, "iteration-picker").is_none());

    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert!(crate::toolbar::shows(
        shell.cx,
        "iteration-picker-label",
        "Iteration 2 of 2"
    ));
    // After the kind's pills.
    let head = crate::shell::bounds(shell.cx, "ref-pill-head");
    assert!(head.right() <= crate::shell::bounds(shell.cx, "iteration-picker").left());
    show(&mut shell, &tab, Choice::Iteration(1));
    assert!(crate::toolbar::shows(
        shell.cx,
        "iteration-picker-label",
        "Iteration 1 of 2"
    ));
}

#[gpui_kit::test]
fn i_opens_the_iteration_menu_at_the_pill(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert!(!tab.read_with(shell.cx, |t, _| iterations::menu_open(t)));
    shell.cx.simulate_keystrokes("i");
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, _| iterations::menu_open(t)));
    // It hangs from the pill's bottom-left corner.
    let pill = crate::shell::bounds(shell.cx, "iteration-picker");
    let menu = crate::shell::bounds(shell.cx, "key-menu");
    assert_eq!((menu.left(), menu.top()), (pill.left(), pill.bottom()));
}

/// Whether the header card's second line reads `text`.
fn byline(shell: &mut Shell, text: &str) -> bool {
    crate::toolbar::shows(shell.cx, "header-byline", text)
}

#[gpui_kit::test]
fn header_card_reloads_after_refresh_and_iteration_switch(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    // An hour after the fixture's epoch: "feature 1" (fixture commit 2)
    // was 58 minutes before, "feature 2" (commit 3) 57.
    let now = (1_767_225_600 + 3_600) * 1000;
    shell
        .cx
        .update(|_, cx| polygloss_app::review_tab::header::set_clock(move || now, cx));
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    assert!(byline(
        &mut shell,
        "1 commit · Polygloss Fixture committed 58m ago"
    ));
    assert!(crate::toolbar::shows(
        shell.cx,
        "header-stats",
        "2 files · +2 −2"
    ));

    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert!(byline(
        &mut shell,
        "2 commits · Polygloss Fixture committed 57m ago"
    ));
    assert!(crate::toolbar::shows(
        shell.cx,
        "header-stats",
        "3 files · +8 −3"
    ));

    show(&mut shell, &tab, Choice::Iteration(1));
    assert!(byline(
        &mut shell,
        "1 commit · Polygloss Fixture committed 58m ago"
    ));
    show(&mut shell, &tab, Choice::Current);
    assert!(byline(
        &mut shell,
        "2 commits · Polygloss Fixture committed 57m ago"
    ));
}

/// The banner strip's context line, if it shows one.
fn context(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, cx| t.banners.read(cx).context().to_string())
}

#[gpui_kit::test]
fn banner_strip_is_empty_on_the_latest_state(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    // What it compares is the header card's now (OQ-39).
    assert_eq!(context(&mut shell, &tab), "");
    assert!(crate::shell::painted(shell.cx, "banner-context").is_none());
    let strip = crate::shell::bounds(shell.cx, "banner-strip");
    assert_eq!(strip.size.height, gpui_kit::px(32.), "still reserved");

    second_commit(&repo);
    refresh(&mut shell, &tab);
    assert_eq!(context(&mut shell, &tab), "", "the new latest state");
    assert!(crate::shell::painted(shell.cx, "banner-context").is_none());
}

#[gpui_kit::test]
fn banner_strip_shows_the_iteration_context_off_latest(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let base = repo.git(&["rev-parse", "--short=7", "main"]);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    let first = repo.git(&["rev-parse", "--short=7", "feature"]);
    second_commit(&repo);
    refresh(&mut shell, &tab);

    show(&mut shell, &tab, Choice::Iteration(1));
    let line = format!("Iteration 1 of 2 · main ({base}) → {first} · 2 files changed");
    assert_eq!(context(&mut shell, &tab), line);
    assert!(crate::shell::painted(shell.cx, &format!("banner-context: {line}")).is_some());

    // Back on the latest state, the strip is empty again.
    show(&mut shell, &tab, Choice::Current);
    assert_eq!(context(&mut shell, &tab), "");
    assert!(crate::shell::painted(shell.cx, "banner-context").is_none());
    assert!(crate::shell::painted(shell.cx, &format!("banner-context: {line}")).is_none());
}

#[gpui_kit::test]
fn iteration_switch_repartitions(cx: &mut gpui_kit::TestAppContext) {
    use crate::categories::{click_band, section_id, sections};
    use polygloss_viewport::ControlAction;

    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/a.rs", numbered("a", 20).as_bytes());
    repo.write("src/b.rs", numbered("b", 20).as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    // Iteration 1: `src/b.rs` and its new test.
    repo.write("src/b.rs", numbered("b", 21).as_bytes());
    repo.write("src/b.test.rs", b"test b\n");
    repo.commit("feature 1");
    let mut shell = start(cx);
    let tab = shell.open(compare_req(&repo)).expect("open the review");
    // Iteration 2: `src/a.rs` and a test directory too.
    repo.write("src/a.rs", numbered("a", 21).as_bytes());
    repo.write("tests/c.rs", b"test c\n");
    repo.commit("feature 2");
    refresh(&mut shell, &tab);
    assert_eq!(
        paths(&mut shell, &tab),
        ["src/a.rs", "src/b.rs", "src/b.test.rs", "tests/c.rs"]
    );
    let tests = section_id(&mut shell, &tab, "tests");
    click_band(&mut shell, &tab, ControlAction::SectionToggle(tests));
    let s = |c: &str, files: &[u32], open: bool| (c.to_owned(), files.to_vec(), open);
    assert_eq!(sections(&mut shell, &tab), [s("tests", &[2, 3], true)]);

    // Iteration 1's files: its test file is in Tests, still open.
    show(&mut shell, &tab, Choice::Iteration(1));
    assert_eq!(paths(&mut shell, &tab), ["src/b.rs", "src/b.test.rs"]);
    assert_eq!(sections(&mut shell, &tab), [s("tests", &[1], true)]);
    let order = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).display_order().to_vec()
    });
    assert_eq!(order, [0, 1]);
}
