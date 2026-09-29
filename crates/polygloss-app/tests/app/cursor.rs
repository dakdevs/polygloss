//! GPUI tests of T3.8: line cursor, ranges, gutter "+", selection and copy,
//! driven through the default key bindings of a review tab (design §11.6,
//! §11.9).

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Entity, Subscription};
use polygloss_app::review_tab::ReviewTab;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{CursorPos, ViewportEvent};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

fn pos(file_idx: u32, side: Side, line: u32) -> CursorPos {
    CursorPos {
        file_idx,
        side,
        line,
        range_start: None,
    }
}

fn cursor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<CursorPos> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor())
}

fn keys(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

/// Every event of the tab's viewport but frame stats, while the
/// subscription lives.
fn record(
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
) -> (Rc<RefCell<Vec<ViewportEvent>>>, Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let sub = shell.cx.update(move |_, cx| {
        cx.subscribe(&viewport, move |_, e: &ViewportEvent, _| {
            if !matches!(e, ViewportEvent::FrameStats(_)) {
                sink.borrow_mut().push(e.clone());
            }
        })
    });
    (events, sub)
}

#[gpui_kit::test]
fn cursor_moves_across_rows_and_files(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(cursor(&mut shell, &tab), None);
    // `src/config.rs` starts with a changed line: removed, then added.
    keys(&mut shell, "j");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 0)));
    keys(&mut shell, "down");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::New, 0)));
    keys(&mut shell, "k");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 0)));
    keys(&mut shell, "up");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 0)));
    // Down through the whole file into the next one.
    for _ in 0..200 {
        keys(&mut shell, "j");
        if cursor(&mut shell, &tab).is_some_and(|c| c.file_idx == 1) {
            break;
        }
    }
    let at = cursor(&mut shell, &tab).expect("a cursor");
    assert_eq!(at.file_idx, 1, "{at:?}");
    keys(&mut shell, "k");
    assert_eq!(cursor(&mut shell, &tab).map(|c| c.file_idx), Some(0));
}

#[gpui_kit::test]
fn shift_arrow_extends_range_on_one_side(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    keys(&mut shell, "j");
    // Old line 0, then (skipping the added row) old line 1.
    keys(&mut shell, "shift-down");
    let want = CursorPos {
        range_start: Some(0),
        ..pos(0, Side::Old, 1)
    };
    assert_eq!(cursor(&mut shell, &tab), Some(want));
    // Back to where it started: no range left.
    keys(&mut shell, "shift-up");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 0)));
    keys(&mut shell, "j");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::New, 0)));
}

#[gpui_kit::test]
fn bracket_keys_jump_between_changes(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    keys(&mut shell, "]");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 0)));
    // The doc comment on line 2 is the next change.
    keys(&mut shell, "]");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 2)));
    keys(&mut shell, "[");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 0)));
}

#[gpui_kit::test]
fn n_p_jump_between_files(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let top =
        |shell: &mut Shell| tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor().file_idx);
    keys(&mut shell, "n");
    assert_eq!(cursor(&mut shell, &tab).map(|c| c.file_idx), Some(1));
    assert_eq!(top(&mut shell), 1);
    keys(&mut shell, "n");
    // `src/main.rs` is added: its first line is new line 0.
    assert_eq!(cursor(&mut shell, &tab), Some(pos(2, Side::New, 0)));
    keys(&mut shell, "p");
    assert_eq!(cursor(&mut shell, &tab).map(|c| c.file_idx), Some(1));
    assert_eq!(top(&mut shell), 1);
}

#[gpui_kit::test]
fn c_on_cursor_emits_comment_requested_with_range(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let (events, _sub) = record(&mut shell, &tab);
    keys(&mut shell, "j");
    keys(&mut shell, "shift-down");
    keys(&mut shell, "c");
    let comments: Vec<ViewportEvent> = events
        .borrow()
        .iter()
        .filter(|e| matches!(e, ViewportEvent::CommentRequested { .. }))
        .cloned()
        .collect();
    assert_eq!(
        comments,
        [ViewportEvent::CommentRequested {
            file_idx: 0,
            side: Side::Old,
            start_line: 0,
            line: 1,
        }]
    );
}

#[gpui_kit::test]
fn cmd_c_copies_source_lines_only(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let _tab = shell.open(compare_req(repo.path())).unwrap();
    keys(&mut shell, "j");
    keys(&mut shell, "shift-down");
    keys(&mut shell, "cmd-c");
    let clip = shell.cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("use std::collections::HashMap;\n\n"));
}

/// A repo whose `x.rs` (100 lines) changes lines 10 and 80: a 63-line gap
/// (old lines 14..77) between the hunks.
fn long_gap_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let old: Vec<String> = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut new = old.clone();
    new[10] = "changed 10\n".to_owned();
    new[80] = "changed 80\n".to_owned();
    repo.write("x.rs", old.concat().as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("x.rs", new.concat().as_bytes());
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

fn expansions(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<(u32, Vec<[u32; 2]>)> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).expansions())
}

#[gpui_kit::test]
fn e_expands_nearest_gap_by_20_lines(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_gap_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    keys(&mut shell, "] ]");
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 80)));
    // The gap above is nearer than the trailing one: its last 20 lines.
    keys(&mut shell, "e");
    assert_eq!(expansions(&mut shell, &tab), [(0, vec![[57, 77]])]);
    // Now the trailing gap is the nearer one: its first 16 (all) lines.
    keys(&mut shell, "e");
    assert_eq!(
        expansions(&mut shell, &tab),
        [(0, vec![[57, 77], [84, 100]])]
    );
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::Old, 80)));
}

#[gpui_kit::test]
fn shift_e_expands_whole_file(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_gap_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    keys(&mut shell, "j");
    keys(&mut shell, "shift-e");
    assert_eq!(expansions(&mut shell, &tab), [(0, vec![[0, 100]])]);
}
