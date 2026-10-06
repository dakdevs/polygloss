//! GPUI tests of T3.9: threads in the diff and the threads panel (design
//! §8.2, §8.4, §8.6, §11.6 "Threads"): placement from carry-forward
//! positions, the thread block (root and flat replies, author, agent,
//! Draft, Question and Outdated badges, resolved threads and agent notes
//! collapsed), "Hide agent notes", the panel, per-file invalidation and
//! `.`/`,` between open threads, both in the viewport's display order
//! (category sections last, T6.15).

use gpui_kit::{Entity, VisualTestContext};
use polygloss_app::keymap::actions::{tab as tab_actions, viewport as viewport_actions};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads::placement::{self, ThreadPlace};
use polygloss_app::threads::{self, ReviewThreads, panel};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, NewThread, OpenRequest, PositionState, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::rows::Layout;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{BlockAnchor, CursorPos};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

mod geometry;
mod sections;

pub fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

pub fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
    }
}

pub fn line(path: &str, side: Side, start_line: u32, line: u32) -> Subject {
    Subject::Line {
        path: path.into(),
        side,
        start_line,
        line,
    }
}

/// Creates a thread on the diff `tab` shows.
pub fn create(
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

pub fn reload(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
}

fn model(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Entity<ReviewThreads> {
    tab.read_with(shell.cx, |t, _| {
        threads::threads(t).cloned().expect("threads are attached")
    })
}

fn painted(cx: &mut VisualTestContext, selector: String) -> bool {
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
        .is_some()
}

fn bounds(cx: &mut VisualTestContext, selector: String) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.debug_bounds(Box::leak(selector.clone().into_boxed_str()))
        .unwrap_or_else(|| panic!("{selector} was not painted"))
}

fn block_ids(shell: &mut Shell, tab: &Entity<ReviewTab>, file: u32) -> Vec<u64> {
    tab.read_with(shell.cx, |t, cx| {
        t.viewport
            .read(cx)
            .document()
            .blocks(file)
            .iter()
            .map(|b| b.id.0)
            .collect()
    })
}

fn cursor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<CursorPos> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor())
}

fn keys(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

#[gpui_kit::test]
fn threads_are_placed_by_their_carry_forward_position(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let on_line = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 3, 5),
        ThreadKind::Comment,
        "range",
        human(),
    );
    let on_file = create(
        &mut shell,
        &tab,
        Subject::File {
            path: "src/greet.ts".into(),
        },
        ThreadKind::Comment,
        "file",
        human(),
    );
    let on_review = create(
        &mut shell,
        &tab,
        Subject::Review,
        ThreadKind::Comment,
        "review",
        human(),
    );
    reload(&mut shell, &tab);
    let m = model(&mut shell, &tab);
    let place = |shell: &mut Shell, id: &str| m.read_with(shell.cx, |m, _| m.place(id).cloned());
    assert_eq!(
        place(&mut shell, &on_line),
        Some(ThreadPlace::Line {
            file_idx: 0,
            side: Side::New,
            start_line: 2,
            line: 4
        })
    );
    assert_eq!(
        place(&mut shell, &on_file),
        Some(ThreadPlace::File { file_idx: 1 })
    );
    assert_eq!(place(&mut shell, &on_review), Some(ThreadPlace::Panel));
    // Blocks: the line thread below its last line, the file thread under
    // the header, the review thread nowhere in the diff.
    let docs = tab.read_with(shell.cx, |t, cx| {
        let doc = t.viewport.read(cx).document();
        (
            doc.blocks(0).to_vec(),
            doc.blocks(1).to_vec(),
            doc.blocks(2).to_vec(),
        )
    });
    assert_eq!(docs.0.len(), 1);
    assert_eq!(docs.0[0].id, placement::block_id(&on_line));
    assert_eq!(
        docs.0[0].anchor,
        BlockAnchor::Line {
            side: Side::New,
            line: 4
        }
    );
    assert_eq!(docs.1.len(), 1);
    assert_eq!(docs.1[0].anchor, BlockAnchor::FileTop);
    assert!(docs.2.is_empty());
    // The review thread is in the panel.
    assert!(painted(shell.cx, format!("threads-panel-{on_review}")));
    // Header and tree flags count open threads per file.
    let flags = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).file_flags().to_vec());
    assert_eq!(flags[0].open_threads, 1);
    assert_eq!(flags[1].open_threads, 1);
    assert_eq!(flags[2].open_threads, 0);
    assert!(!flags[0].agent_threads);
}

#[gpui_kit::test]
fn thread_block_shows_root_replies_and_badges(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let id = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 1, 1),
        ThreadKind::Comment,
        "Why **BTreeMap**?",
        agent(),
    );
    let reply = shell.core.reply(&id, "Sorted output.", &human()).unwrap();
    reload(&mut shell, &tab);
    let root = model(&mut shell, &tab).read_with(shell.cx, |m, _| {
        m.thread(&id).expect("loaded").comments[0].id.clone()
    });
    assert!(painted(shell.cx, format!("thread-{id}")));
    assert!(painted(shell.cx, format!("thread-comment-{root}")));
    assert!(painted(shell.cx, format!("thread-comment-{reply}")));
    // The agent's root has an agent badge; the human's reply is a draft.
    assert!(painted(shell.cx, format!("thread-agent-{root}")));
    assert!(!painted(shell.cx, format!("thread-draft-{root}")));
    assert!(painted(shell.cx, format!("thread-draft-{reply}")));
    assert!(!painted(shell.cx, format!("thread-agent-{reply}")));
    // Replies are flat: below the root, same left edge.
    let a = bounds(shell.cx, format!("thread-comment-{root}"));
    let b = bounds(shell.cx, format!("thread-comment-{reply}"));
    assert!(b.origin.y > a.origin.y);
    assert_eq!(a.origin.x, b.origin.x);
}

#[gpui_kit::test]
fn agent_note_collapsed_and_hidden_by_toggle(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let note = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 1, 1),
        ThreadKind::Note,
        "This swaps the map type.\n\nMore detail below.",
        agent(),
    );
    let comment = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 3, 3),
        ThreadKind::Comment,
        "A comment.",
        human(),
    );
    reload(&mut shell, &tab);
    let m = model(&mut shell, &tab);
    let root = m.read_with(shell.cx, |m, _| {
        m.thread(&note).unwrap().comments[0].id.clone()
    });
    // Collapsed to a one-line chip: no body.
    assert!(painted(shell.cx, format!("thread-chip-{note}")));
    assert!(!painted(shell.cx, format!("thread-comment-{root}")));
    let chip = bounds(shell.cx, format!("thread-chip-{note}"));
    let card = bounds(shell.cx, format!("thread-{comment}"));
    assert!(chip.size.height < card.size.height, "{chip:?} vs {card:?}");
    // A click expands it.
    shell
        .cx
        .simulate_click(chip.center(), gpui_kit::Modifiers::none());
    draw(shell.cx);
    assert!(painted(shell.cx, format!("thread-comment-{root}")));
    assert!(m.read_with(shell.cx, |m, _| m.is_expanded(&note)));

    // "Hide agent notes" removes the note from the diff and the panel.
    let (note_block, comment_block) = (
        placement::block_id(&note).0,
        placement::block_id(&comment).0,
    );
    assert_eq!(block_ids(&mut shell, &tab, 0), [note_block, comment_block]);
    shell.cx.dispatch_action(tab_actions::ToggleAgentNotes);
    draw(shell.cx);
    assert!(m.read_with(shell.cx, |m, _| m.hide_agent_notes()));
    assert_eq!(block_ids(&mut shell, &tab, 0), [comment_block]);
    assert!(!painted(shell.cx, format!("thread-chip-{note}")));
    assert!(!painted(shell.cx, format!("threads-panel-{note}")));
    assert!(painted(shell.cx, format!("threads-panel-{comment}")));
    // And back.
    shell.cx.dispatch_action(tab_actions::ToggleAgentNotes);
    draw(shell.cx);
    assert_eq!(block_ids(&mut shell, &tab, 0), [note_block, comment_block]);
    assert!(painted(shell.cx, format!("threads-panel-{note}")));
}

#[gpui_kit::test]
fn question_badge_visible(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let question = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Question,
        "Should keys be case-insensitive?",
        agent(),
    );
    reload(&mut shell, &tab);
    let root = model(&mut shell, &tab).read_with(shell.cx, |m, _| {
        m.thread(&question).unwrap().comments[0].id.clone()
    });
    // Expanded (not a chip), with the question badge, in the diff and the panel.
    assert!(!painted(shell.cx, format!("thread-chip-{question}")));
    assert!(painted(shell.cx, format!("thread-comment-{root}")));
    assert!(painted(shell.cx, format!("thread-question-{question}")));
    assert!(painted(
        shell.cx,
        format!("threads-panel-question-{question}")
    ));
    let flags = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).file_flags()[0]);
    assert_eq!(flags.open_threads, 1);
    assert!(flags.agent_threads);
}

#[gpui_kit::test]
fn outdated_thread_inline_with_snippet_and_in_panel(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    repo.git(&["checkout", "-q", "-b", "topic", "refs/tags/head"]);
    let mut shell = start(cx);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/heads/topic".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    // Iteration 1, and an agent thread on `let mut entries = BTreeMap::new();`.
    let first = shell.core.open(&req).unwrap();
    let blobs = BlobReader::open(&first.repo).unwrap();
    let id = shell
        .core
        .create_thread(
            &NewThread {
                review_id: first.review_id.clone(),
                diff_id: first.diff_id.clone(),
                subject: line("src/config.rs", Side::New, 10, 10),
                kind: ThreadKind::Comment,
                body_md: "Pre-size this map?".into(),
                author: agent(),
            },
            &blobs,
        )
        .unwrap();
    // The branch moves: that line changes.
    let changed = crate::support::CONFIG_RS_HEAD.replace(
        "let mut entries = BTreeMap::new();",
        "let mut entries: BTreeMap<String, String> = BTreeMap::new();",
    );
    repo.write("src/config.rs", changed.as_bytes());
    repo.commit("pre-size");
    let tab = shell.open(req).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    assert_ne!(
        tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone()),
        first.diff_id
    );
    let m = model(&mut shell, &tab);
    let state = m.read_with(shell.cx, |m, _| m.position(&id).map(|p| p.state));
    assert_eq!(state, Some(PositionState::Outdated));
    // Inline at the nearest mapped line, with the badge and the original
    // snippet, and listed in the panel.
    let place = m.read_with(shell.cx, |m, _| m.place(&id).cloned());
    assert!(
        matches!(
            place,
            Some(ThreadPlace::Line {
                file_idx: 0,
                side: Side::New,
                ..
            })
        ),
        "{place:?}"
    );
    assert_eq!(block_ids(&mut shell, &tab, 0), [placement::block_id(&id).0]);
    tab.update(shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| {
            v.scroll_to(
                polygloss_viewport::ScrollTarget::Block(placement::block_id(&id)),
                cx,
            )
        })
    });
    draw(shell.cx);
    assert!(painted(shell.cx, format!("thread-outdated-{id}")));
    assert!(painted(shell.cx, format!("thread-snippet-{id}")));
    assert!(painted(shell.cx, format!("threads-panel-{id}")));
    assert!(painted(shell.cx, format!("threads-panel-outdated-{id}")));
    // The snippet is the original text, not the current one.
    let snippet = m.read_with(shell.cx, |m, _| {
        m.thread(&id)
            .unwrap()
            .anchor
            .anchor_snippet
            .clone()
            .unwrap()
    });
    assert!(snippet.contains("let mut entries = BTreeMap::new();"));
}

#[gpui_kit::test]
fn split_thread_in_side_column_with_spacer(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    shell.cx.dispatch_action(viewport_actions::LayoutSplit);
    draw(shell.cx);
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout()),
        Layout::Split
    );
    // Old line 1 (`use std::collections::HashMap;`) and new line 5.
    let old = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::Old, 1, 1),
        ThreadKind::Comment,
        "Old side.",
        human(),
    );
    let new = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Comment,
        "New side.",
        human(),
    );
    reload(&mut shell, &tab);
    let pane = bounds(shell.cx, "viewport-pane".into());
    let mid = pane.origin.x + pane.size.width / 2.;
    let old_card = bounds(shell.cx, format!("thread-{old}"));
    let new_card = bounds(shell.cx, format!("thread-{new}"));
    // Each in its side's column.
    assert!(old_card.origin.x >= pane.origin.x, "{old_card:?} {pane:?}");
    assert!(
        old_card.right() <= mid + gpui_kit::px(1.),
        "{old_card:?} mid {mid:?}"
    );
    assert!(
        new_card.origin.x >= mid - gpui_kit::px(1.),
        "{new_card:?} mid {mid:?}"
    );
    assert!(new_card.right() <= pane.right() + gpui_kit::px(1.));
    // Each block row is as tall as its card (plus margins), with the other
    // side a same-height spacer: the rows below move down by the block.
    let debug = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).debug());
    let row = |name: String| {
        let i = debug
            .visible_rows
            .iter()
            .position(|r| *r == name)
            .unwrap_or_else(|| panic!("{name} not in {:#?}", debug.visible_rows));
        debug.row_bounds[i]
    };
    let (_, old_h) = row(format!("[block {}]", placement::block_id(&old).0));
    let (_, new_h) = row(format!("[block {}]", placement::block_id(&new).0));
    assert!(
        old_h >= old_card.size.height.as_f32(),
        "{old_h} vs {old_card:?}"
    );
    assert!(
        new_h >= new_card.size.height.as_f32(),
        "{new_h} vs {new_card:?}"
    );
    assert!(old_h < old_card.size.height.as_f32() + 24.);
}

#[gpui_kit::test]
fn thread_update_invalidates_one_file_only(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let a = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 3, 3),
        ThreadKind::Comment,
        "A",
        human(),
    );
    let b = create(
        &mut shell,
        &tab,
        line("src/main.rs", Side::New, 2, 2),
        ThreadKind::Comment,
        "B",
        human(),
    );
    reload(&mut shell, &tab);
    let m = model(&mut shell, &tab);
    let stats = |shell: &mut Shell| m.read_with(shell.cx, |m, _| m.stats().clone());
    // What the viewport itself did: how often each file's blocks were
    // replaced (each re-lays that file out), whoever asked.
    let relayouts = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            let doc = t.viewport.read(cx).document();
            (0..doc.len())
                .map(|f| doc.block_sets(f))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        stats(&mut shell)
            .set_blocks
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(relayouts(&mut shell), [1, 0, 1]);

    // A reply changes A's content only: its block is re-measured, no file
    // is re-laid out.
    m.update(shell.cx, |m, _| m.reset_stats());
    shell.core.reply(&a, "A reply", &human()).unwrap();
    reload(&mut shell, &tab);
    let s = stats(&mut shell);
    assert!(s.set_blocks.is_empty(), "{s:?}");
    assert_eq!(s.invalidated, [placement::block_id(&a)]);
    assert_eq!(relayouts(&mut shell), [1, 0, 1], "no file re-laid out");

    // A new thread in `src/config.rs` re-lays out that file only.
    m.update(shell.cx, |m, _| m.reset_stats());
    create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 10, 10),
        ThreadKind::Comment,
        "C",
        human(),
    );
    reload(&mut shell, &tab);
    let s = stats(&mut shell);
    assert_eq!(
        s.set_blocks.keys().copied().collect::<Vec<_>>(),
        [0],
        "{s:?}"
    );
    assert!(!s.invalidated.contains(&placement::block_id(&b)));
    assert_eq!(relayouts(&mut shell), [2, 0, 1], "src/config.rs only");

    // Resolving a thread changes its block only.
    let c = create(
        &mut shell,
        &tab,
        line("src/main.rs", Side::New, 4, 4),
        ThreadKind::Comment,
        "An agent's",
        agent(),
    );
    reload(&mut shell, &tab);
    m.update(shell.cx, |m, _| m.reset_stats());
    shell
        .core
        .set_resolved(&c, true, &Actor::human(), None)
        .unwrap();
    reload(&mut shell, &tab);
    let s = stats(&mut shell);
    assert!(s.set_blocks.is_empty(), "{s:?}");
    assert_eq!(s.invalidated, [placement::block_id(&c)]);
    assert_eq!(
        relayouts(&mut shell),
        [2, 0, 2],
        "resolving re-lays out nothing"
    );
}

#[gpui_kit::test]
fn dot_and_comma_jump_between_open_threads_across_files(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    // A: on a line hidden between two hunks of `src/config.rs` (new 16).
    let a = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 16, 16),
        ThreadKind::Comment,
        "A",
        agent(),
    );
    // B: resolved, in `src/greet.ts`: skipped.
    let b = create(
        &mut shell,
        &tab,
        line("src/greet.ts", Side::New, 3, 3),
        ThreadKind::Comment,
        "B",
        agent(),
    );
    shell
        .core
        .set_resolved(&b, true, &Actor::human(), None)
        .unwrap();
    // C: a range in `src/main.rs`, which is collapsed.
    let c = create(
        &mut shell,
        &tab,
        line("src/main.rs", Side::New, 2, 4),
        ThreadKind::Question,
        "C",
        agent(),
    );
    reload(&mut shell, &tab);
    tab.update(shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| v.set_collapsed(2, true, cx))
    });
    draw(shell.cx);
    let at_a = CursorPos {
        file_idx: 0,
        side: Side::New,
        line: 15,
        range_start: None,
    };
    let at_c = CursorPos {
        file_idx: 2,
        side: Side::New,
        line: 3,
        range_start: Some(1),
    };
    let hidden_text = "if let Some((key, value)) = line.split_once('=') {";
    let shown = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).debug())
            .visible_rows
            .iter()
            .any(|r| r.contains(hidden_text))
    };
    assert!(!shown(&mut shell), "line 16 starts hidden in a gap");

    keys(&mut shell, ".");
    assert_eq!(cursor(&mut shell, &tab), Some(at_a));
    assert!(shown(&mut shell), "the gap was expanded around line 16");
    keys(&mut shell, ".");
    assert_eq!(cursor(&mut shell, &tab), Some(at_c), "B is resolved");
    assert!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).collapsed())
            .is_empty(),
        "src/main.rs was expanded"
    );
    assert!(painted(shell.cx, format!("thread-{c}")));
    // Wraps around, and back.
    keys(&mut shell, ".");
    assert_eq!(cursor(&mut shell, &tab), Some(at_a));
    keys(&mut shell, ",");
    assert_eq!(cursor(&mut shell, &tab), Some(at_c));
    keys(&mut shell, ",");
    assert_eq!(cursor(&mut shell, &tab), Some(at_a));
    assert!(painted(shell.cx, format!("thread-{a}")));
    // The panel's rows jump too.
    let row = bounds(shell.cx, format!("threads-panel-{c}"));
    shell
        .cx
        .simulate_click(row.center(), gpui_kit::Modifiers::none());
    draw(shell.cx);
    assert_eq!(cursor(&mut shell, &tab), Some(at_c));

    mixed_sides_follow_the_diff_order(&mut shell);
}

/// `.`/`,` and the panel order threads on both sides of one file as the
/// diff shows them, not by line number (review fix): old and new line
/// numbers are different coordinates.
fn mixed_sides_follow_the_diff_order(shell: &mut Shell) {
    // `list.txt` gains 30 lines at the top and loses old line 40, which
    // shows at about new line 70.
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let base: String = (1..=50).map(|i| format!("b{i}\n")).collect();
    let head: String = (1..=30)
        .map(|i| format!("n{i}\n"))
        .chain((1..=50).filter(|&i| i != 40).map(|i| format!("b{i}\n")))
        .collect();
    repo.write("list.txt", base.as_bytes());
    repo.write("z.txt", b"z1\nz2\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("list.txt", head.as_bytes());
    repo.write("z.txt", b"z1\nZ2\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    shell.cx.dispatch_action(viewport_actions::LayoutUnified);
    draw(shell.cx);
    // X: removed old line 40 (row ~70). Y: new line 50 (old 20, row 50).
    // Z: in the next file.
    let x = create(
        shell,
        &tab,
        line("list.txt", Side::Old, 40, 40),
        ThreadKind::Comment,
        "X",
        human(),
    );
    let y = create(
        shell,
        &tab,
        line("list.txt", Side::New, 50, 50),
        ThreadKind::Comment,
        "Y",
        human(),
    );
    let z = create(
        shell,
        &tab,
        line("z.txt", Side::New, 2, 2),
        ThreadKind::Comment,
        "Z",
        human(),
    );
    reload(shell, &tab);
    let at = |file_idx, side, line| CursorPos {
        file_idx,
        side,
        line,
        range_start: None,
    };
    let (at_x, at_y, at_z) = (
        at(0, Side::Old, 39),
        at(0, Side::New, 49),
        at(1, Side::New, 1),
    );
    let put = |shell: &mut Shell, pos: CursorPos| {
        tab.update(shell.cx, |t, cx| {
            t.viewport.update(cx, |v, cx| v.set_cursor(Some(pos), cx))
        });
        draw(shell.cx);
        assert_eq!(cursor(shell, &tab), Some(pos));
    };
    // The panel lists them top to bottom: Y (row 50), X (row ~70), Z.
    let m = model(shell, &tab);
    let rows = m.read_with(shell.cx, panel::rows);
    let ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, [y.as_str(), x.as_str(), z.as_str()]);

    // From the top of the file: Y, then X below it, then Z; and back.
    put(shell, at(0, Side::New, 0));
    keys(shell, ".");
    assert_eq!(cursor(shell, &tab), Some(at_y));
    keys(shell, ".");
    assert_eq!(cursor(shell, &tab), Some(at_x));
    keys(shell, ".");
    assert_eq!(cursor(shell, &tab), Some(at_z));
    keys(shell, ",");
    assert_eq!(cursor(shell, &tab), Some(at_x));
    keys(shell, ",");
    assert_eq!(cursor(shell, &tab), Some(at_y));
    // New line 66 (old 37) is above X although 66 > 40: `.` goes to X.
    put(shell, at(0, Side::New, 65));
    keys(shell, ".");
    assert_eq!(cursor(shell, &tab), Some(at_x));
    put(shell, at(0, Side::New, 65));
    keys(shell, ",");
    assert_eq!(cursor(shell, &tab), Some(at_y));

    // Split pairs rows, and the cursor may be on the old side.
    shell.cx.dispatch_action(viewport_actions::LayoutSplit);
    draw(shell.cx);
    put(shell, at(0, Side::Old, 36));
    keys(shell, ".");
    assert_eq!(cursor(shell, &tab), Some(at_x));
    keys(shell, ".");
    assert_eq!(cursor(shell, &tab), Some(at_z));
    put(shell, at(0, Side::Old, 36));
    keys(shell, ",");
    assert_eq!(cursor(shell, &tab), Some(at_y));
}

/// Threads on both sides of a file the viewport has not loaded order by
/// the changed blocks computed with the threads (review fix, round 2), and
/// again after the diff options change.
#[gpui_kit::test]
fn old_side_threads_order_in_files_not_loaded(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    // 40 long files first, so `list.txt` (as in the mixed-sides case) is
    // far below the fold.
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let base: String = (1..=50).map(|i| format!("b{i}\n")).collect();
    let head: String = (1..=30)
        .map(|i| format!("n{i}\n"))
        .chain((1..=50).filter(|&i| i != 40).map(|i| format!("b{i}\n")))
        .collect();
    let filler = |tag: &str| -> String { (1..=40).map(|i| format!("{tag}{i}\n")).collect() };
    for f in 0..40 {
        repo.write(&format!("a{f:02}.txt"), filler("old").as_bytes());
    }
    repo.write("list.txt", base.as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for f in 0..40 {
        repo.write(&format!("a{f:02}.txt"), filler("new").as_bytes());
    }
    repo.write("list.txt", head.as_bytes());
    repo.commit("head");
    repo.git(&["tag", "head"]);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    shell.cx.dispatch_action(viewport_actions::LayoutUnified);
    draw(shell.cx);
    // X: removed old line 40 (row ~70). Y: new line 50 (old 20, row 50).
    let x = create(
        &mut shell,
        &tab,
        line("list.txt", Side::Old, 40, 40),
        ThreadKind::Comment,
        "X",
        human(),
    );
    let y = create(
        &mut shell,
        &tab,
        line("list.txt", Side::New, 50, 50),
        ThreadKind::Comment,
        "Y",
        human(),
    );
    reload(&mut shell, &tab);
    let list_loaded = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            matches!(
                t.viewport.read(cx).document().state(40),
                polygloss_viewport::FileState::Materialized(_)
            )
        })
    };
    let m = model(&mut shell, &tab);
    let ids = |shell: &mut Shell| -> Vec<String> {
        m.read_with(shell.cx, panel::rows)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    };
    assert!(!list_loaded(&mut shell), "list.txt is not loaded");
    // By line number X (40) would come first; the diff shows Y first.
    assert_eq!(ids(&mut shell), [y.clone(), x.clone()]);

    // Hiding whitespace changes the diff options: the cached blocks are
    // recomputed with them (stale ones are never used).
    shell.cx.dispatch_action(viewport_actions::ToggleWhitespace);
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).options().diff.ignore_whitespace
    }));
    assert!(!list_loaded(&mut shell), "list.txt is still not loaded");
    assert_eq!(ids(&mut shell), [y, x]);
}

#[test]
fn diff_order_puts_both_sides_in_one_coordinate() {
    use placement::{line_diff_order, line_order};
    // Old 0..2 unchanged (new 0..2); old 2..4 replaced by new 2..5; old
    // 4..6 unchanged (new 5..7); old 6 deleted; old 7.. unchanged (new 7..);
    // 3 lines inserted before old 9 (new 9..12).
    let changes = [(2..4, 2..5), (6..7, 7..7), (9..9, 9..12)];
    let c = Some(&changes[..]);
    let u = Layout::Unified;
    // Unified: removed lines, then added ones, at the block's row.
    assert_eq!(line_order(c, u, Side::Old, 3), (2, 0, 1));
    assert_eq!(line_order(c, u, Side::New, 2), (2, 1, 0));
    assert!(line_order(c, u, Side::Old, 3) < line_order(c, u, Side::New, 2));
    // Unchanged lines share their row on both sides.
    assert_eq!(
        line_order(c, u, Side::Old, 5),
        line_order(c, u, Side::New, 6)
    );
    assert_eq!(line_order(c, u, Side::Old, 1), (1, 1, 0));
    // A deletion sorts between the lines around it.
    let deleted = line_order(c, u, Side::Old, 6);
    assert!(line_order(c, u, Side::New, 6) < deleted);
    assert!(deleted < line_order(c, u, Side::New, 7));
    assert!(deleted < line_order(c, u, Side::Old, 7));
    // Lines inserted before old 9 come before it.
    assert!(line_order(c, u, Side::New, 11) < line_order(c, u, Side::Old, 9));
    assert_eq!(line_order(c, u, Side::Old, 9), (12, 1, 0));
    // Split pairs a block's lines row by row, old first on a shared row.
    let s = Layout::Split;
    assert_eq!(
        line_order(c, s, Side::Old, 3),
        line_order(c, s, Side::New, 3)
    );
    assert!(line_diff_order(0, Side::Old, 3, c, s) < line_diff_order(0, Side::New, 3, c, s));
    assert!(line_order(c, s, Side::New, 2) < line_order(c, s, Side::Old, 3));
    // Without the diff, lines order by number.
    assert_eq!(line_order(None, u, Side::Old, 40), (40, 1, 0));
    // Header threads first, panel-only threads last.
    assert!(
        placement::diff_order(&ThreadPlace::File { file_idx: 7 }, 0, c, u)
            < line_diff_order(0, Side::Old, 0, c, u)
    );
    assert!(
        line_diff_order(3, Side::New, 9, c, u)
            < placement::diff_order(&ThreadPlace::Panel, 0, c, u)
    );
}

#[gpui_kit::test]
fn threads_of_a_fresh_review_load_on_open(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo: FixtureRepo = code_change_repo();
    let mut shell = start(cx);
    // Threads made before the tab opens show without a reload.
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();
    let blobs = BlobReader::open(&opened.repo).unwrap();
    let id = shell
        .core
        .create_thread(
            &NewThread {
                review_id: opened.review_id.clone(),
                diff_id: opened.diff_id.clone(),
                subject: line("src/main.rs", Side::New, 1, 1),
                kind: ThreadKind::Comment,
                body_md: "Hello".into(),
                author: agent(),
            },
            &blobs,
        )
        .unwrap();
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let m = model(&mut shell, &tab);
    assert!(m.read_with(shell.cx, |m, _| m.is_loaded()));
    assert_eq!(block_ids(&mut shell, &tab, 2), [placement::block_id(&id).0]);
    assert!(painted(shell.cx, format!("threads-panel-{id}")));
}

#[gpui_kit::test]
fn threads_panel_rows_are_cards_on_the_canvas(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    let ids = [1, 5].map(|n| {
        let subject = line("src/config.rs", Side::New, n, n);
        create(
            &mut shell,
            &tab,
            subject,
            ThreadKind::Comment,
            "Hm.",
            human(),
        )
    });
    reload(&mut shell, &tab);
    let pane = bounds(shell.cx, "threads-pane".into());
    let rows = ids.map(|id| bounds(shell.cx, format!("threads-panel-{id}")));
    // Cards: inset from the panel's edges (`SIDEBAR`, T7.7; exact in
    // `threads::geometry`) and apart from each other.
    for row in rows {
        assert!(
            row.left() - pane.left() >= gpui_kit::px(10.),
            "{row:?} in {pane:?}"
        );
        assert!(
            pane.right() - row.right() >= gpui_kit::px(10.),
            "{row:?} in {pane:?}"
        );
    }
    assert!(
        rows[1].top() - rows[0].bottom() >= gpui_kit::px(8.),
        "{rows:?}"
    );
    assert!(painted(shell.cx, "threads-panel-count".into()));
}
