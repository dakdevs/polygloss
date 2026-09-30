//! GPUI tests of T3.15: find across all files (⌘F, design §11.14). The
//! search reads every file's blobs in the background (loaded into the
//! viewport or not), streams its matches into the find bar, and `⏎`/`⇧⏎`
//! walk them, revealing hidden context, expanding collapsed files and
//! loading generated or large ones on the way.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::{Entity, Focusable as _, TestAppContext};
use polygloss_app::find::search::{self, FindMatch, FindOptions};
use polygloss_app::find::{self, FindBar};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::settings::SettingsStore;
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, ObjectFormat, Oid, Side};
use polygloss_viewport::{BodyRow, CursorPos, DiffProvider, FileState, RowKey};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox};

/// Files in [`many_files_repo`].
const FILES: usize = 40;
/// Lines per file in [`many_files_repo`].
const LINES: usize = 150;

fn numbered(tag: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{tag} line {i}\n")).collect()
}

/// `f00.txt` … `f39.txt`, 150 lines each, line 0 changed in every file, so
/// the viewport materializes only the first few. The last file also has
/// `needle` on hidden context (line 50, both sides), on an added line
/// (new 100) and on a removed one (old 120).
fn many_files_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let base = |i: usize| {
        let mut lines = numbered(&format!("f{i:02}"), LINES);
        if i == FILES - 1 {
            lines[50] = "needle context\n".into();
            lines[120] = "needle removed\n".into();
        }
        lines
    };
    for i in 0..FILES {
        repo.write(&format!("f{i:02}.txt"), base(i).concat().as_bytes());
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for i in 0..FILES {
        let mut lines = base(i);
        lines[0] = format!("f{i:02} changed\n");
        if i == FILES - 1 {
            lines[100] = "needle added\n".into();
            lines[120] = "f39 replaced\n".into();
        }
        repo.write(&format!("f{i:02}.txt"), lines.concat().as_bytes());
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

fn pos(file_idx: u32, side: Side, line: u32) -> CursorPos {
    CursorPos {
        file_idx,
        side,
        line,
        range_start: None,
    }
}

fn bar(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Entity<FindBar> {
    tab.read_with(shell.cx, |t, _| find::find_bar(t).cloned())
        .expect("every review tab has a find bar")
}

fn keys(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

/// ⌘F, then `query` typed into the find field.
fn find_text(shell: &mut Shell, query: &str) {
    keys(shell, "cmd-f");
    shell.cx.simulate_input(query);
    draw(shell.cx);
}

/// `(file, side, line)` of every match.
fn found(shell: &mut Shell, bar: &Entity<FindBar>) -> Vec<(u32, Side, u32)> {
    bar.read_with(shell.cx, |b, _| {
        b.matches()
            .iter()
            .map(|m| (m.file_idx, m.side, m.line))
            .collect()
    })
}

fn cursor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<CursorPos> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor())
}

fn materialized(shell: &mut Shell, tab: &Entity<ReviewTab>, f: u32) -> bool {
    tab.read_with(shell.cx, |t, cx| {
        matches!(
            t.viewport.read(cx).document().state(f),
            FileState::Materialized(_)
        )
    })
}

/// Whether file `f`'s layout has a code row showing `line` of `side`.
fn shows_line(shell: &mut Shell, tab: &Entity<ReviewTab>, f: u32, side: Side, line: u32) -> bool {
    tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        let Some(layout) = v.document().file_layout(f) else {
            return false;
        };
        let Some(r) = layout.find(RowKey::Line { side, line }) else {
            return false;
        };
        match layout.rows()[r] {
            BodyRow::Line { old, new, .. } => match side {
                Side::Old => old == Some(line),
                Side::New => new == Some(line),
            },
            _ => false,
        }
    })
}

#[gpui_kit::test]
fn find_searches_unloaded_files(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = many_files_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let last = (FILES - 1) as u32;
    assert!(
        !materialized(&mut shell, &tab, last),
        "the last file is far below the viewport"
    );
    let bar = bar(&mut shell, &tab);
    assert!(!bar.read_with(shell.cx, |b, _| b.is_open()));

    assert!(shell.cx.debug_bounds("tree-count").is_some());
    find_text(&mut shell, "needle");
    assert!(bar.read_with(shell.cx, |b, _| b.is_open()));
    // Find takes the left pane in place of the file tree.
    assert!(shell.cx.debug_bounds("find-pane").is_some());
    assert!(shell.cx.debug_bounds("tree-count").is_none());
    assert!(shell.cx.debug_bounds("find-match-2").is_some());
    // Context shows once (on the new side), in display order: the
    // context line, the added line, the removed line.
    assert_eq!(
        found(&mut shell, &bar),
        [
            (last, Side::New, 50),
            (last, Side::New, 100),
            (last, Side::Old, 120)
        ]
    );
    let (searching, label, hidden, old_line) = bar.read_with(shell.cx, |b, _| {
        (
            b.is_searching(),
            b.status_label(),
            b.matches()[0].hidden,
            b.matches()[0].old_line,
        )
    });
    assert!(!searching);
    assert_eq!(label, "3 matches");
    assert!(hidden, "line 50 is between the hunks");
    assert_eq!(old_line, Some(50));
    // Searching read the blobs itself; the viewport loaded nothing.
    assert!(!materialized(&mut shell, &tab, last));

    // The result list names the file once, then its three matches.
    let rows = bar.read_with(shell.cx, |b, _| b.list_rows());
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert_eq!(rows[0], find::ListRow::File(last));
    let previews: Vec<String> = bar.read_with(shell.cx, |b, _| {
        b.matches().iter().map(|m| m.preview.to_string()).collect()
    });
    assert_eq!(
        previews,
        ["needle context", "needle added", "needle removed"]
    );
}

#[gpui_kit::test]
fn find_next_prev_wraps(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = many_files_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    let last = (FILES - 1) as u32;
    find_text(&mut shell, "needle");
    assert_eq!(bar.read_with(shell.cx, |b, _| b.current()), None);

    let want = [
        (0, pos(last, Side::New, 50)),
        (1, pos(last, Side::New, 100)),
        (2, pos(last, Side::Old, 120)),
        // Past the last match: back to the first.
        (0, pos(last, Side::New, 50)),
    ];
    for (ix, at) in want {
        keys(&mut shell, "enter");
        assert_eq!(bar.read_with(shell.cx, |b, _| b.current()), Some(ix));
        assert_eq!(cursor(&mut shell, &tab), Some(at));
        assert!(shows_line(&mut shell, &tab, at.file_idx, at.side, at.line));
        let top = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor().file_idx);
        assert_eq!(top, last, "the match is scrolled into view");
    }
    assert_eq!(bar.read_with(shell.cx, |b, _| b.status_label()), "1 of 3");
    // Before the first match: to the last.
    keys(&mut shell, "shift-enter");
    assert_eq!(bar.read_with(shell.cx, |b, _| b.current()), Some(2));
    assert_eq!(cursor(&mut shell, &tab), Some(pos(last, Side::Old, 120)));
    keys(&mut shell, "shift-enter");
    assert_eq!(bar.read_with(shell.cx, |b, _| b.current()), Some(1));
    assert_eq!(cursor(&mut shell, &tab), Some(pos(last, Side::New, 100)));

    // The find field keeps the keyboard while walking the matches; Escape
    // closes the bar and gives it back to the diff, cursor on the match.
    let input_focused = shell.cx.update(|window, cx| {
        bar.read(cx)
            .input()
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    });
    assert!(input_focused);
    keys(&mut shell, "escape");
    assert!(!bar.read_with(shell.cx, |b, _| b.is_open()));
    assert!(shell.cx.debug_bounds("find-pane").is_none());
    assert!(shell.cx.debug_bounds("tree-count").is_some());
    let diff_focused = shell
        .cx
        .update(|window, cx| tab.read(cx).viewport_focus().is_focused(window));
    assert!(diff_focused);
    assert_eq!(cursor(&mut shell, &tab), Some(pos(last, Side::New, 100)));
}

#[gpui_kit::test]
fn find_marks_matches_in_the_diff_while_open(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = case_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    let highlights = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            t.viewport.read(cx).find_highlights().cloned()
        })
    };
    assert!(highlights(&mut shell).is_none(), "no marks before ⌘F");

    // A query marks its matches: the viewport gets the search's own matcher
    // (case-insensitive by default), nothing is current yet.
    find_text(&mut shell, "foo");
    let h = highlights(&mut shell).expect("marks while find is open");
    assert!(h.current.is_none());
    assert_eq!((h.matcher)(b"Foo bar foo"), vec![0..3, 8..11]);
    assert!((h.matcher)(b"bar").is_empty());
    // Every match found has its byte range in its line.
    let ranges: Vec<(u32, std::ops::Range<usize>)> = bar.read_with(shell.cx, |b, _| {
        b.matches()
            .iter()
            .map(|m| (m.line, m.range.clone()))
            .collect()
    });
    assert_eq!(ranges, [(1, 0..3), (2, 0..3), (3, 0..3)]);

    // ⏎ emphasizes the match gone to.
    keys(&mut shell, "enter");
    keys(&mut shell, "enter");
    let current = highlights(&mut shell).and_then(|h| h.current);
    let current = current.expect("the current match is emphasized");
    assert_eq!(
        (current.file_idx, current.side, current.line, current.range),
        (0, Side::New, 2, 0..3)
    );

    // A case-sensitive search marks what it finds.
    bar.update(shell.cx, |b, cx| b.set_case_sensitive(true, cx));
    draw(shell.cx);
    let h = highlights(&mut shell).unwrap();
    assert_eq!((h.matcher)(b"Foo bar foo"), vec![8..11]);
    assert!(h.current.is_none(), "a new search starts over");

    // Esc closes find and clears the marks; ⌘F brings them back.
    keys(&mut shell, "escape");
    assert!(highlights(&mut shell).is_none());
    keys(&mut shell, "cmd-f");
    assert!(highlights(&mut shell).is_some());
    // An empty or invalid query marks nothing.
    bar.update_in(shell.cx, |b, window, cx| b.set_query("", window, cx));
    draw(shell.cx);
    assert!(highlights(&mut shell).is_none());
    bar.update(shell.cx, |b, cx| b.set_regex(true, cx));
    bar.update_in(shell.cx, |b, window, cx| b.set_query("(", window, cx));
    draw(shell.cx);
    assert!(highlights(&mut shell).is_none());
}

/// `x.rs`, 100 lines, lines 10 and 80 changed: old lines 14..77 are hidden
/// between the hunks.
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

#[gpui_kit::test]
fn find_match_in_collapsed_context_expands(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_gap_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let expansions = |shell: &mut Shell| viewport.read_with(shell.cx, |v, _| v.expansions());
    assert!(expansions(&mut shell).is_empty());
    assert!(!shows_line(&mut shell, &tab, 0, Side::New, 40));

    find_text(&mut shell, "line 40");
    assert_eq!(found(&mut shell, &bar), [(0, Side::New, 40)]);
    keys(&mut shell, "enter");
    let revealed = expansions(&mut shell);
    assert_eq!(revealed.len(), 1, "{revealed:?}");
    let [start, end] = revealed[0].1[0];
    assert!(start <= 40 && 40 < end, "{revealed:?}");
    assert!(end - start < 20, "only around the match: {revealed:?}");
    assert!(shows_line(&mut shell, &tab, 0, Side::New, 40));
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::New, 40)));

    // A match that is already shown reveals nothing more.
    keys(&mut shell, "enter");
    assert_eq!(expansions(&mut shell), revealed);

    // A match in a collapsed file expands it.
    shell
        .cx
        .update(|_, cx| viewport.update(cx, |v, cx| v.set_collapsed(0, true, cx)));
    draw(shell.cx);
    keys(&mut shell, "enter");
    assert!(!viewport.read_with(shell.cx, |v, _| v.document().is_collapsed(0)));
    assert!(shows_line(&mut shell, &tab, 0, Side::New, 40));
}

/// `Cargo.lock` (generated, collapsed) and `src/big.rs` (60 changed lines:
/// large with the threshold at 50), both with `serde` far down, and
/// `src/lib.rs`.
fn generated_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("Cargo.lock", b"# lock\n");
    repo.write("src/lib.rs", b"pub fn lib() {}\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    let lock: String = (0..40)
        .map(|i| format!("[[package]]\nname = \"dep{i}\"\n"))
        .collect::<String>()
        + "[[package]]\nname = \"serde\"\n";
    repo.write("Cargo.lock", lock.as_bytes());
    let big: String = (0..60)
        .map(|i| format!("fn f{i}() {{}}\n"))
        .collect::<String>()
        + "use serde::Serialize;\n";
    repo.write("src/big.rs", big.as_bytes());
    repo.write("src/lib.rs", b"pub fn lib() -> u32 { 1 }\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn find_lists_matches_in_generated_files(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = generated_repo();
    let mut shell = start(cx);
    shell.cx.update(|_, cx| {
        let mut settings = (*SettingsStore::global(cx).settings()).clone();
        settings.diff.large_file_changed_lines = 50;
        SettingsStore::set(settings, cx);
    });
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    let files = tab.read_with(shell.cx, |t, _| t.opened.files.clone());
    let paths: Vec<&str> = files.iter().map(|f| f.display_path()).collect();
    assert_eq!(paths, ["Cargo.lock", "src/big.rs", "src/lib.rs"]);
    assert!(files[0].generated);
    let bar = bar(&mut shell, &tab);

    find_text(&mut shell, "serde");
    assert_eq!(
        found(&mut shell, &bar),
        [(0, Side::New, 81), (1, Side::New, 60)]
    );
    let rows = bar.read_with(shell.cx, |b, _| b.list_rows());
    assert_eq!(
        rows,
        [
            find::ListRow::File(0),
            find::ListRow::Match(0),
            find::ListRow::File(1),
            find::ListRow::Match(1),
        ]
    );
    // Neither shows rows yet: a generated placeholder and "Load diff".
    assert!(!shows_line(&mut shell, &tab, 0, Side::New, 81));
    assert!(!shows_line(&mut shell, &tab, 1, Side::New, 60));

    // Going to a match loads the file's diff.
    keys(&mut shell, "enter");
    draw(shell.cx);
    assert!(shows_line(&mut shell, &tab, 0, Side::New, 81));
    assert_eq!(cursor(&mut shell, &tab), Some(pos(0, Side::New, 81)));
    keys(&mut shell, "enter");
    draw(shell.cx);
    assert!(shows_line(&mut shell, &tab, 1, Side::New, 60));
    assert_eq!(cursor(&mut shell, &tab), Some(pos(1, Side::New, 60)));
}

/// `a.txt` with `Foo bar`, `foo baz` and `fooo` added.
fn case_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"start\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("a.txt", b"start\nFoo bar\nfoo baz\nfooo\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn find_case_and_regex_toggles(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = case_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    let lines = |shell: &mut Shell| -> Vec<u32> {
        found(shell, &bar).into_iter().map(|(_, _, l)| l).collect()
    };

    // Case-insensitive, literal by default.
    find_text(&mut shell, "Foo");
    assert_eq!(lines(&mut shell), [1, 2, 3]);
    // ⌥⌘C: match case.
    keys(&mut shell, "alt-cmd-c");
    assert!(bar.read_with(shell.cx, |b, _| b.options().case_sensitive));
    assert_eq!(lines(&mut shell), [1]);
    // The toggle button turns it off again.
    let at = shell
        .cx
        .debug_bounds("find-case-toggle")
        .expect("the match-case toggle")
        .center();
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);
    assert!(!bar.read_with(shell.cx, |b, _| b.options().case_sensitive));
    assert_eq!(lines(&mut shell), [1, 2, 3]);

    // A pattern is literal text until regex is on (⌥⌘R or the button).
    shell
        .cx
        .update(|window, cx| bar.update(cx, |b, cx| b.set_query("fo+ ba[rz]", window, cx)));
    draw(shell.cx);
    assert_eq!(lines(&mut shell), Vec::<u32>::new());
    assert_eq!(
        bar.read_with(shell.cx, |b, _| b.status_label()),
        "No matches"
    );
    keys(&mut shell, "alt-cmd-r");
    assert!(bar.read_with(shell.cx, |b, _| b.options().regex));
    assert_eq!(lines(&mut shell), [1, 2]);
    let at = shell
        .cx
        .debug_bounds("find-regex-toggle")
        .expect("the regex toggle")
        .center();
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);
    assert!(!bar.read_with(shell.cx, |b, _| b.options().regex));
    keys(&mut shell, "alt-cmd-r");

    // An invalid pattern says so and finds nothing.
    shell
        .cx
        .update(|window, cx| bar.update(cx, |b, cx| b.set_query("fo(", window, cx)));
    draw(shell.cx);
    assert_eq!(lines(&mut shell), Vec::<u32>::new());
    assert!(bar.read_with(shell.cx, |b, _| b.error().is_some()));
    assert_eq!(
        bar.read_with(shell.cx, |b, _| b.status_label()),
        "Invalid pattern"
    );
}

#[gpui_kit::test]
fn find_cancels_on_new_query(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = many_files_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    keys(&mut shell, "cmd-f");
    // Two queries before any background work runs: the first search is
    // cancelled and never reports.
    shell.cx.update(|window, cx| {
        bar.update(cx, |b, cx| {
            b.set_query("changed", window, cx);
            b.set_query("f39 line 7", window, cx);
        })
    });
    draw(shell.cx);
    let stats = bar.read_with(shell.cx, |b, _| b.stats());
    assert_eq!(stats.started, 2);
    assert_eq!(stats.completed, 1);
    // The first search's background jobs were told to stop.
    assert_eq!(stats.cancelled, 1);
    // `f39 line 7` and `f39 line 70` … `79`, all in the last file.
    let got = found(&mut shell, &bar);
    assert_eq!(got.len(), 11, "{got:?}");
    assert!(got.iter().all(|&(f, _, _)| f == (FILES - 1) as u32));

    // Clearing the query cancels too and empties the list.
    shell
        .cx
        .update(|window, cx| bar.update(cx, |b, cx| b.set_query("", window, cx)));
    draw(shell.cx);
    assert!(found(&mut shell, &bar).is_empty());
    assert_eq!(bar.read_with(shell.cx, |b, _| b.status_label()), "");
    // Nothing was running: nothing more to cancel.
    assert_eq!(bar.read_with(shell.cx, |b, _| b.stats().cancelled), 1);
}

/// Files in [`wide_repo`]: more than one round of background jobs.
const WIDE: usize = 300;

/// `w000.txt` … `w299.txt`, one changed line each; `needle` in the first
/// and the last.
fn wide_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for i in 0..WIDE {
        repo.write(&format!("w{i:03}.txt"), b"base\n");
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for i in 0..WIDE {
        let text = if i == 0 || i == WIDE - 1 {
            "needle\n"
        } else {
            "head\n"
        };
        repo.write(&format!("w{i:03}.txt"), text.as_bytes());
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn find_streams_results_before_finishing(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = wide_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    // Every state the bar shows: (matches, searching, label).
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _watch = shell.cx.update(|_, cx| {
        let seen = seen.clone();
        cx.observe(&bar, move |bar, cx| {
            let b = bar.read(cx);
            seen.borrow_mut()
                .push((b.matches().len(), b.is_searching(), b.status_label()));
        })
    });
    find_text(&mut shell, "needle");
    let seen = seen.borrow().clone();
    assert!(
        seen.iter()
            .any(|(n, searching, label)| *n == 1 && *searching && label == "1 match…"),
        "the first file's match shows while the rest is searched: {seen:?}"
    );
    assert_eq!(
        seen.last().map(|s| (s.0, s.1)),
        Some((2, false)),
        "{seen:?}"
    );
    assert_eq!(
        found(&mut shell, &bar),
        [(0, Side::New, 0), ((WIDE - 1) as u32, Side::New, 0)]
    );
}

/// `a.txt` with `needle` re-indented (whitespace only) and `b` changed.
fn reindent_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"  needle\na\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("a.txt", b"    needle\nb\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn find_reruns_when_diff_options_change(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = reindent_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    find_text(&mut shell, "needle");
    // Whitespace shown: a removed and an added line.
    assert_eq!(
        found(&mut shell, &bar),
        [(0, Side::Old, 0), (0, Side::New, 0)]
    );
    keys(&mut shell, "enter");
    assert_eq!(bar.read_with(shell.cx, |b, _| b.current()), Some(0));
    let started = bar.read_with(shell.cx, |b, _| b.stats().started);

    // Hiding whitespace makes the line context: the search runs again, so
    // no match points at the removed row that is gone.
    shell.cx.update(|_, cx| {
        let mut settings = (*SettingsStore::global(cx).settings()).clone();
        settings.diff.hide_whitespace = true;
        SettingsStore::set(settings, cx);
    });
    draw(shell.cx);
    assert_eq!(
        bar.read_with(shell.cx, |b, _| b.stats().started),
        started + 1
    );
    assert_eq!(found(&mut shell, &bar), [(0, Side::New, 0)]);
    let (old_line, current) =
        bar.read_with(shell.cx, |b, _| (b.matches()[0].old_line, b.current()));
    assert_eq!(old_line, Some(0));
    assert_eq!(current, None);

    // Anything else the viewport does (moving the cursor) searches nothing.
    keys(&mut shell, "enter");
    assert_eq!(
        bar.read_with(shell.cx, |b, _| b.stats().started),
        started + 1
    );
}

#[gpui_kit::test]
fn find_first_enter_starts_at_the_cursor(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_gap_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let put_cursor = |shell: &mut Shell, at: CursorPos| {
        shell
            .cx
            .update(|_, cx| viewport.update(cx, |v, cx| v.set_cursor(Some(at), cx)));
        draw(shell.cx);
        assert_eq!(cursor(shell, &tab), Some(at));
    };
    let search = |shell: &mut Shell, q: &str| {
        shell.cx.update(|window, cx| {
            bar.update(cx, |b, cx| {
                b.set_query("", window, cx);
                b.set_query(q, window, cx);
            })
        });
        draw(shell.cx);
    };
    let current = |shell: &mut Shell| bar.read_with(shell.cx, |b, _| b.current());
    keys(&mut shell, "cmd-f");

    // `changed 10` and `changed 80`; the cursor on new line 12, between.
    search(&mut shell, "changed");
    assert_eq!(
        found(&mut shell, &bar),
        [(0, Side::New, 10), (0, Side::New, 80)]
    );
    put_cursor(&mut shell, pos(0, Side::New, 12));
    keys(&mut shell, "enter");
    assert_eq!(
        current(&mut shell),
        Some(1),
        "the first match after the cursor"
    );
    search(&mut shell, "changed");
    put_cursor(&mut shell, pos(0, Side::New, 12));
    keys(&mut shell, "shift-enter");
    assert_eq!(
        current(&mut shell),
        Some(0),
        "the last match before the cursor"
    );

    // Removed `line 10` shows before `changed 10`: from the added line, the
    // next match is the added line itself, the previous one the removed.
    search(&mut shell, "10");
    assert_eq!(
        found(&mut shell, &bar),
        [(0, Side::Old, 10), (0, Side::New, 10)]
    );
    put_cursor(&mut shell, pos(0, Side::New, 10));
    keys(&mut shell, "enter");
    assert_eq!(current(&mut shell), Some(1));
    search(&mut shell, "10");
    put_cursor(&mut shell, pos(0, Side::New, 10));
    keys(&mut shell, "shift-enter");
    assert_eq!(current(&mut shell), Some(0));
    // Past the last match: wraps to the first.
    search(&mut shell, "changed");
    put_cursor(&mut shell, pos(0, Side::New, 83));
    keys(&mut shell, "enter");
    assert_eq!(current(&mut shell), Some(0));
}

/// The pure search over two blobs: context lines once (on the new side,
/// with their old line), removed lines before the lines that replace
/// them, matches inside a line in order, and hidden context flagged.
#[test]
fn search_blobs_orders_by_display_and_dedupes_context() {
    let old = b"a needle\nkeep\nold needle\nb\nc\nd\ne\nf\ng\nh\nctx needle\n";
    let new = b"a needle\nkeep\nnew needle needle\nb\nc\nd\ne\nf\ng\nh\nctx needle\n";
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    /// `(side, line, old_line, hidden, preview_match)`.
    type Row = (Side, u32, Option<u32>, bool, std::ops::Range<usize>);
    let got: Vec<Row> = search::search_blobs(7, old, new, &re, &DiffOptions::default(), usize::MAX)
        .into_iter()
        .map(|m: FindMatch| {
            assert_eq!(m.file_idx, 7);
            (m.side, m.line, m.old_line, m.hidden, m.preview_match)
        })
        .collect();
    assert_eq!(
        got,
        [
            (Side::New, 0, Some(0), false, 2..8),
            (Side::Old, 2, None, false, 4..10),
            (Side::New, 2, None, false, 4..10),
            (Side::New, 2, None, false, 11..17),
            // Old line 10 is more than 3 lines past the change: hidden.
            (Side::New, 10, Some(10), true, 4..10),
        ]
    );
}

/// Long lines are cut around the match (marked `…`); indentation is
/// dropped without a mark.
#[test]
fn search_previews_cut_long_lines_around_the_match() {
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    let indented = b"        let needle = 1;\n";
    let long = format!("{}needle{}\n", "x".repeat(300), "y".repeat(300));
    let got = search::search_blobs(0, b"", indented, &re, &DiffOptions::default(), usize::MAX);
    assert_eq!(got[0].preview.as_ref(), "let needle = 1;");
    assert_eq!(got[0].preview_match, 4..10);
    let got = search::search_blobs(
        0,
        b"",
        long.as_bytes(),
        &re,
        &DiffOptions::default(),
        usize::MAX,
    );
    let p = got[0].preview.as_ref();
    assert!(p.starts_with('…') && p.ends_with('…'), "{p}");
    assert_eq!(&p[got[0].preview_match.clone()], "needle");
    assert!(p.len() < 200, "{}", p.len());
}

#[test]
fn search_compile_handles_case_regex_and_errors() {
    let opts = |case_sensitive, regex| FindOptions {
        case_sensitive,
        regex,
    };
    assert!(search::compile("", opts(false, false)).unwrap().is_none());
    let lit = search::compile("a.b", opts(false, false)).unwrap().unwrap();
    assert!(lit.is_match(b"A.B"));
    assert!(!lit.is_match(b"axb"));
    let cased = search::compile("a.b", opts(true, false)).unwrap().unwrap();
    assert!(!cased.is_match(b"A.B"));
    let re = search::compile("a.b", opts(false, true)).unwrap().unwrap();
    assert!(re.is_match(b"AXB"));
    assert!(search::compile("a(", opts(false, true)).is_err());
}

/// `^` and `$` are a line's ends in regex mode, on lines past the first,
/// before a final newline and before a CRLF (the whole-blob pre-check
/// agrees with the per-line pass).
#[test]
fn search_regex_anchors_match_at_every_line() {
    let re = |q: &str| {
        search::compile(
            q,
            FindOptions {
                case_sensitive: false,
                regex: true,
            },
        )
        .unwrap()
        .unwrap()
    };
    let lines = |q: &str, new: &[u8]| -> Vec<u32> {
        search::search_blobs(0, b"", new, &re(q), &DiffOptions::default(), usize::MAX)
            .into_iter()
            .map(|m| m.line)
            .collect()
    };
    let new = b"use x;\nfn main() {\n    let y = 1;\n}\n";
    assert_eq!(lines("^fn", new), [1]);
    assert_eq!(lines(";$", new), [0, 2]);
    assert_eq!(lines(r"\{$", new), [1]);
    assert_eq!(lines("^}$", new), [3]);
    let crlf = b"use x;\r\nfn main() {\r\n}\r\n";
    assert_eq!(lines("^fn", crlf), [1]);
    assert_eq!(lines(";$", crlf), [0]);
    assert_eq!(lines(r"\{$", crlf), [1]);
    // Still never across lines.
    assert_eq!(lines(r";\sfn", new), Vec::<u32>::new());
}

/// A limit keeps the first matches in display order (removed lines before
/// the lines replacing them) and stops collecting there.
#[test]
fn search_blobs_stops_at_the_limit_in_display_order() {
    let re = search::compile("e", FindOptions::default())
        .unwrap()
        .unwrap();
    let old = b"keep\nold one\nold three\nctx\n";
    let new = b"keep\nnew one\nnew two\nctx\n";
    let at = |limit| -> Vec<(Side, u32)> {
        search::search_blobs(0, old, new, &re, &DiffOptions::default(), limit)
            .into_iter()
            .map(|m| (m.side, m.line))
            .collect()
    };
    let all = at(usize::MAX);
    assert_eq!(
        all,
        [
            (Side::New, 0),
            (Side::New, 0),
            (Side::Old, 1),
            (Side::Old, 2),
            (Side::Old, 2),
            (Side::New, 1),
            (Side::New, 1),
            (Side::New, 2),
        ]
    );
    for limit in 0..all.len() {
        assert_eq!(at(limit), all[..limit], "limit {limit}");
    }
}

/// An in-memory provider of one text file.
struct OneFile {
    files: Arc<Vec<FileChange>>,
    old: Arc<[u8]>,
    new: Arc<[u8]>,
}

impl DiffProvider for OneFile {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }
    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        Ok(if *oid == self.files[0].old_blob {
            self.old.clone()
        } else {
            self.new.clone()
        })
    }
    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        Ok(self.load_blob(oid)?.len() as u64)
    }
}

#[test]
fn search_file_skips_binary_and_stops_when_cancelled() {
    let change = |kind| FileChange {
        idx: 0,
        status: FileStatus::Modified,
        old_path: Some(GitPath::from_bytes(b"a.txt")),
        new_path: Some(GitPath::from_bytes(b"a.txt")),
        old_mode: None,
        new_mode: None,
        old_blob: Oid::parse(&"1".repeat(40), ObjectFormat::Sha1).unwrap(),
        new_blob: Oid::parse(&"2".repeat(40), ObjectFormat::Sha1).unwrap(),
        similarity: None,
        kind,
        generated: false,
    };
    let provider = |kind, new: &[u8]| OneFile {
        files: Arc::new(vec![change(kind)]),
        old: Arc::from(&b"x\n"[..]),
        new: Arc::from(new),
    };
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    let diff = DiffOptions::default();
    let go = |p: &OneFile, cancel: bool| {
        search::search_file(
            &p.files[0],
            p,
            &re,
            &diff,
            &AtomicBool::new(cancel),
            usize::MAX,
        )
        .len()
    };
    assert_eq!(go(&provider(FileKind::Text, b"needle\n"), false), 1);
    assert_eq!(go(&provider(FileKind::Text, b"needle\n"), true), 0);
    assert_eq!(go(&provider(FileKind::Binary, b"needle\n"), false), 0);
    // Listed as text, but a NUL byte: binary after all.
    assert_eq!(go(&provider(FileKind::Text, b"needle\0\n"), false), 0);
    // A limit caps the matches kept.
    let p = provider(FileKind::Text, b"needle\nneedle\nneedle\n");
    let limited = |limit| {
        search::search_file(&p.files[0], &p, &re, &diff, &AtomicBool::new(false), limit).len()
    };
    assert_eq!(limited(usize::MAX), 3);
    assert_eq!(limited(2), 2);
    assert_eq!(limited(0), 0);
}

/// Many files of one provider, each `needle` twice; the cancel flag is set
/// while the third file is read.
struct CancelOnThird {
    files: Arc<Vec<FileChange>>,
    cancel: Arc<AtomicBool>,
    loads: std::sync::atomic::AtomicUsize,
}

impl DiffProvider for CancelOnThird {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }
    fn load_blob(&self, _: &Oid) -> anyhow::Result<Arc<[u8]>> {
        use std::sync::atomic::Ordering;
        // Two blobs per file.
        if self.loads.fetch_add(1, Ordering::SeqCst) == 4 {
            self.cancel.store(true, Ordering::SeqCst);
        }
        Ok(Arc::from(&b"needle\nneedle\n"[..]))
    }
    fn blob_size(&self, _: &Oid) -> anyhow::Result<u64> {
        Ok(14)
    }
}

/// One background job's work: files in order, stopping when cancelled or
/// once `limit` matches are kept.
#[test]
fn search_chunk_stops_when_cancelled_or_at_the_limit() {
    let files: Vec<FileChange> = (0..6u32)
        .map(|i| FileChange {
            idx: i,
            status: FileStatus::Modified,
            old_path: Some(GitPath::from_bytes(b"a.txt")),
            new_path: Some(GitPath::from_bytes(b"a.txt")),
            old_mode: None,
            new_mode: None,
            old_blob: Oid::parse(&format!("{:040x}", 2 * i + 1), ObjectFormat::Sha1).unwrap(),
            new_blob: Oid::parse(&format!("{:040x}", 2 * i + 2), ObjectFormat::Sha1).unwrap(),
            similarity: None,
            kind: FileKind::Text,
            generated: false,
        })
        .collect();
    let re = search::compile("needle", FindOptions::default())
        .unwrap()
        .unwrap();
    let diff = DiffOptions::default();
    let provider = || {
        let cancel = Arc::new(AtomicBool::new(false));
        CancelOnThird {
            files: Arc::new(files.clone()),
            cancel,
            loads: Default::default(),
        }
    };
    let ids = |got: Vec<FindMatch>| -> Vec<u32> { got.into_iter().map(|m| m.file_idx).collect() };

    // Cancelled while reading the third file: nothing from it on.
    let p = provider();
    let got = search::search_chunk(&files, &p, &re, &diff, &p.cancel, usize::MAX);
    assert_eq!(ids(got), [0, 0, 1, 1]);

    // Not cancelled: stops once the limit is reached, mid-file too.
    let p = provider();
    let never = AtomicBool::new(false);
    let got = search::search_chunk(&files, &p, &re, &diff, &never, 5);
    assert_eq!(ids(got), [0, 0, 1, 1, 2]);
    assert_eq!(
        p.loads.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "no file read past the limit"
    );
}

#[gpui_kit::test]
fn find_previews_draw_in_the_diff_code_font_without_ligatures(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = case_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bar = bar(&mut shell, &tab);
    find_text(&mut shell, "foo");
    let code_font = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).code_font().clone());
    let preview = bar.read_with(shell.cx, |b, cx| b.preview_font(cx));
    // Result previews use the diff's font, features included, so `->` in a
    // preview is drawn as typed like it is in the diff (buffer_font.ligatures).
    assert_eq!(preview, code_font);
    assert_eq!(
        preview.features,
        polygloss_viewport::code_font_features(false),
        "ligatures are off by default"
    );
}
