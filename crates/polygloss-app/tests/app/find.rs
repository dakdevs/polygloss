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
    let got: Vec<Row> = search::search_blobs(7, old, new, &re, &DiffOptions::default())
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
    let got = search::search_blobs(0, b"", indented, &re, &DiffOptions::default());
    assert_eq!(got[0].preview.as_ref(), "let needle = 1;");
    assert_eq!(got[0].preview_match, 4..10);
    let got = search::search_blobs(0, b"", long.as_bytes(), &re, &DiffOptions::default());
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
        search::search_file(&p.files[0], p, &re, &diff, &AtomicBool::new(cancel)).len()
    };
    assert_eq!(go(&provider(FileKind::Text, b"needle\n"), false), 1);
    assert_eq!(go(&provider(FileKind::Text, b"needle\n"), true), 0);
    assert_eq!(go(&provider(FileKind::Binary, b"needle\n"), false), 0);
    // Listed as text, but a NUL byte: binary after all.
    assert_eq!(go(&provider(FileKind::Text, b"needle\0\n"), false), 0);
}
