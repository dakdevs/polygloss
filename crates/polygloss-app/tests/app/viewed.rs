//! GPUI tests of T3.7: Viewed UX (design §9, ADR-0022). `v` in the viewport
//! and the tree, the header and tree checkboxes, folder tri-state and "Mark
//! folder viewed", the toolbar's `N/M` (T6.8), carry-over across reopening
//! and iterations, the "changed since viewed" badge, and never pinning.

use std::time::Duration;

use gpui_kit::{Entity, TestAppContext, px};
use polygloss_app::keymap::actions::tree as tree_actions;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::tree::row::Check;
use polygloss_app::tree::{FileTree, file_tree};
use polygloss_app::viewed;
use polygloss_core::git::{Since, Source};
use polygloss_core::review::{OpenRequest, ViewedState};
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{FileFlags, ViewportEvent};

use crate::shell::{Shell, bounds, compare_req, draw, hover, painted, start, unhover};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};
use crate::toolbar::shows;

use ViewedState::{ChangedSinceViewed, NotViewed, Viewed};

/// `n` lines of numbered text, so every file is taller than the window.
fn lines(tag: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{tag} line {i}\n")).collect()
}

/// A repo with tags `base` and `head`; the diff, in order: 0 `README.md`,
/// 1 `docs/intro.md`, 2 `src/a.rs`, 3 `src/b.rs`, 4 `src/c.rs` (all
/// modified, each taller than the window).
fn five_file_repo() -> FixtureRepo {
    const PATHS: [&str; 5] = [
        "README.md",
        "docs/intro.md",
        "src/a.rs",
        "src/b.rs",
        "src/c.rs",
    ];
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for p in PATHS {
        repo.write(p, lines(p, 80).as_bytes());
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for p in PATHS {
        repo.write(p, (String::from("// head\n") + &lines(p, 80)).as_bytes());
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

fn keys(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

fn focus_viewport(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    shell.cx.update(|window, cx| {
        let focus = tab.read(cx).viewport_focus().clone();
        window.focus(&focus, cx);
    });
    draw(shell.cx);
}

fn tree_of(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Entity<FileTree> {
    tab.read_with(shell.cx, |t, _| file_tree(t).cloned().expect("a file tree"))
}

/// The store's Viewed states of the tab's files.
fn stored(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<ViewedState> {
    let (id, files) = tab.read_with(shell.cx, |t, _| {
        (t.review_id.clone(), t.opened.files.clone())
    });
    shell.core.viewed_states(Some(&id), &files).unwrap()
}

/// The tab's Viewed states as the app holds them.
fn shown(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<ViewedState> {
    tab.read_with(shell.cx, |t, _| {
        viewed::states(t).expect("viewed is attached").to_vec()
    })
}

/// The flags of the viewport's headers and of the tree's rows (they agree).
fn flags(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<FileFlags> {
    let tree = tree_of(shell, tab);
    let (viewport, tree) = tab.read_with(shell.cx, |t, cx| {
        (
            t.viewport.read(cx).file_flags().to_vec(),
            tree.read(cx).file_flags().to_vec(),
        )
    });
    assert_eq!(
        viewport, tree,
        "the viewport and the tree show the same flags"
    );
    viewport
}

fn viewed_flags(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<bool> {
    flags(shell, tab).iter().map(|f| f.viewed).collect()
}

fn collapsed(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<u32> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).collapsed())
}

fn top_file(shell: &mut Shell, tab: &Entity<ReviewTab>) -> u32 {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor().file_idx)
}

fn cursor_file(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<u32> {
    tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).cursor().map(|c| c.file_idx)
    })
}

fn progress(shell: &mut Shell, tab: &Entity<ReviewTab>) -> (usize, usize) {
    tab.read_with(shell.cx, |t, _| viewed::progress(t))
}

/// Clicks a file header's Viewed checkbox (the viewport reports it).
fn header_checkbox(shell: &mut Shell, tab: &Entity<ReviewTab>, idx: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |_, cx| cx.emit(ViewportEvent::ViewedToggled(idx)));
    draw(shell.cx);
}

fn click(shell: &mut Shell, selector: &'static str) {
    let at = shell
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element {selector}"))
        .center();
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);
}

/// Closes the active tab (⌘W) and opens `req` again.
fn reopen(shell: &mut Shell, tab: &Entity<ReviewTab>, req: OpenRequest) -> Entity<ReviewTab> {
    focus_viewport(shell, tab);
    keys(shell, "cmd-w");
    assert_eq!(shell.tabs().0, 1, "only Home is left");
    let tab = shell.open(req).unwrap();
    draw(shell.cx);
    tab
}

#[gpui_kit::test]
fn v_marks_viewed_collapses_and_jumps_to_next_unviewed(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = five_file_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // `docs/intro.md` was viewed elsewhere (the key is global): it opens
    // viewed and collapsed.
    let intro = tab.read_with(shell.cx, |t, _| t.opened.files[1].clone());
    shell.core.set_viewed(None, &intro, true).unwrap();
    tab.update(shell.cx, viewed::reload);
    draw(shell.cx);
    assert_eq!(
        shown(&mut shell, &tab),
        [NotViewed, Viewed, NotViewed, NotViewed, NotViewed]
    );
    assert_eq!(
        viewed_flags(&mut shell, &tab),
        [false, true, false, false, false]
    );

    // `v` on the first file: viewed in the store, collapsed, and the next
    // unviewed file (skipping `docs/intro.md`) comes to the top with the
    // cursor on it.
    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "v");
    assert_eq!(
        stored(&mut shell, &tab),
        [Viewed, Viewed, NotViewed, NotViewed, NotViewed]
    );
    assert_eq!(
        viewed_flags(&mut shell, &tab),
        [true, true, false, false, false]
    );
    assert!(collapsed(&mut shell, &tab).contains(&0));
    assert_eq!(top_file(&mut shell, &tab), 2);
    assert_eq!(cursor_file(&mut shell, &tab), Some(2));
    let highlighted = tree_of(&mut shell, &tab).read_with(shell.cx, |t, _| t.highlighted_file());
    assert_eq!(highlighted, Some(2));

    // Again: the cursor's file, then the next one.
    keys(&mut shell, "v");
    assert_eq!(stored(&mut shell, &tab)[2], Viewed);
    assert_eq!(top_file(&mut shell, &tab), 3);
    assert_eq!(cursor_file(&mut shell, &tab), Some(3));

    // The header checkbox of a file further down does the same.
    header_checkbox(&mut shell, &tab, 4);
    assert_eq!(stored(&mut shell, &tab)[4], Viewed);
    assert!(collapsed(&mut shell, &tab).contains(&4));
    // The next unviewed file wraps around to file 3.
    assert_eq!(top_file(&mut shell, &tab), 3);

    // `v` in the tree toggles the selected file.
    let tree = tree_of(&mut shell, &tab);
    shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.focus(window, cx)));
    tree.update(shell.cx, |t, cx| t.select_file(3, cx));
    draw(shell.cx);
    keys(&mut shell, "v");
    assert_eq!(stored(&mut shell, &tab), [Viewed; 5]);
    // Nothing unviewed is left: no jump.
    assert_eq!(top_file(&mut shell, &tab), 3);

    // Unchecking expands the file again, without jumping.
    header_checkbox(&mut shell, &tab, 0);
    assert_eq!(stored(&mut shell, &tab)[0], NotViewed);
    assert!(!collapsed(&mut shell, &tab).contains(&0));
    assert_eq!(
        viewed_flags(&mut shell, &tab),
        [false, true, true, true, true]
    );
    assert_eq!(top_file(&mut shell, &tab), 3);
}

#[gpui_kit::test]
fn viewed_survives_reopen_and_new_iteration_when_unchanged(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    header_checkbox(&mut shell, &tab, 0);
    header_checkbox(&mut shell, &tab, 2);
    assert_eq!(shown(&mut shell, &tab), [Viewed, NotViewed, Viewed]);

    // Reopened: the marks come back from the store, viewed files collapsed
    // (as GitHub does).
    let tab = reopen(&mut shell, &tab, compare_req(repo.path()));
    assert_eq!(shown(&mut shell, &tab), [Viewed, NotViewed, Viewed]);
    assert_eq!(viewed_flags(&mut shell, &tab), [true, false, true]);
    assert_eq!(collapsed(&mut shell, &tab), [0, 2]);

    // A new iteration that changes only `src/greet.ts`: the other two keep
    // their blob pairs, so they stay viewed.
    repo.git(&["checkout", "-q", "refs/tags/head"]);
    repo.write("src/greet.ts", b"export const greet = () => \"hi\";\n");
    repo.commit("head 2");
    repo.git(&["tag", "-f", "head"]);
    let review_id = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let tab = reopen(&mut shell, &tab, compare_req(repo.path()));
    assert_eq!(
        tab.read_with(shell.cx, |t, _| t.review_id.clone()),
        review_id
    );
    assert_eq!(shell.core.iterations(&review_id).unwrap().len(), 2);
    assert_eq!(shown(&mut shell, &tab), [Viewed, NotViewed, Viewed]);
    assert_eq!(viewed_flags(&mut shell, &tab), [true, false, true]);
    assert_eq!(progress(&mut shell, &tab), (2, 3));
}

#[gpui_kit::test]
fn viewed_clears_and_badges_when_file_changes(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    header_checkbox(&mut shell, &tab, 0);
    header_checkbox(&mut shell, &tab, 1);
    assert_eq!(shown(&mut shell, &tab), [Viewed, Viewed, NotViewed]);

    // `src/config.rs` changes in a new iteration: unchecked, with the
    // "changed since viewed" badge in the header and the tree, and shown
    // expanded; `src/greet.ts` did not change and stays viewed.
    repo.git(&["checkout", "-q", "refs/tags/head"]);
    let config = std::fs::read_to_string(repo.path().join("src/config.rs")).unwrap();
    repo.write("src/config.rs", (config + "\n// more\n").as_bytes());
    repo.commit("head 2");
    repo.git(&["tag", "-f", "head"]);
    let tab = reopen(&mut shell, &tab, compare_req(repo.path()));
    assert_eq!(
        shown(&mut shell, &tab),
        [ChangedSinceViewed, Viewed, NotViewed]
    );
    let f = flags(&mut shell, &tab);
    assert!(!f[0].viewed && f[0].changed_since_viewed);
    assert!(f[1].viewed && !f[1].changed_since_viewed);
    assert_eq!(collapsed(&mut shell, &tab), [1]);
    assert_eq!(progress(&mut shell, &tab), (1, 3));
    assert!(
        shell.cx.debug_bounds("tree-changed-0").is_some(),
        "the tree's dot"
    );

    // Marking it viewed again clears the badge.
    header_checkbox(&mut shell, &tab, 0);
    let f = flags(&mut shell, &tab);
    assert!(f[0].viewed && !f[0].changed_since_viewed);
    assert_eq!(stored(&mut shell, &tab), [Viewed, Viewed, NotViewed]);
}

#[gpui_kit::test]
fn folder_tristate_and_mark_folder_viewed(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = five_file_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let tree = tree_of(&mut shell, &tab);
    let check = |shell: &mut Shell, dir: &str| tree.read_with(shell.cx, |t, _| t.folder_check(dir));
    assert_eq!(check(&mut shell, "src"), Some(Check::Off));

    // The folder's checkbox marks every file below it viewed and collapses
    // them; the folder shows checked.
    click(&mut shell, "tree-check-d:src");
    assert_eq!(
        stored(&mut shell, &tab),
        [NotViewed, NotViewed, Viewed, Viewed, Viewed]
    );
    assert_eq!(collapsed(&mut shell, &tab), [2, 3, 4]);
    assert_eq!(check(&mut shell, "src"), Some(Check::On));
    // The viewport was not showing them: it stays where it is.
    assert_eq!(top_file(&mut shell, &tab), 0);

    // One file unchecked: the folder is mixed.
    click(&mut shell, "tree-check-f:3");
    assert_eq!(stored(&mut shell, &tab)[3], NotViewed);
    assert_eq!(check(&mut shell, "src"), Some(Check::Mixed));

    // "Mark folder viewed" on the selected folder checks the rest.
    click(&mut shell, "tree-row-d:src");
    click(&mut shell, "tree-row-d:src"); // expanded again
    let selected = tree.read_with(shell.cx, |t, _| t.selected_dir().map(|d| d.0));
    assert_eq!(selected.as_deref(), Some("src"));
    shell.cx.dispatch_action(tree_actions::MarkFolderViewed);
    draw(shell.cx);
    assert_eq!(
        stored(&mut shell, &tab),
        [NotViewed, NotViewed, Viewed, Viewed, Viewed]
    );
    assert_eq!(check(&mut shell, "src"), Some(Check::On));

    // A fully viewed folder's checkbox unchecks every file.
    click(&mut shell, "tree-check-d:src");
    assert_eq!(stored(&mut shell, &tab), [NotViewed; 5]);
    assert_eq!(check(&mut shell, "src"), Some(Check::Off));
    assert!(collapsed(&mut shell, &tab).is_empty());

    // With a file selected, "Mark folder viewed" marks its folder.
    tree.update(shell.cx, |t, cx| t.select_file(1, cx));
    draw(shell.cx);
    shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.focus(window, cx)));
    shell.cx.dispatch_action(tree_actions::MarkFolderViewed);
    draw(shell.cx);
    assert_eq!(
        stored(&mut shell, &tab),
        [NotViewed, Viewed, NotViewed, NotViewed, NotViewed]
    );
    assert_eq!(check(&mut shell, "docs"), Some(Check::On));
    // The viewport showed it: the next unviewed file comes up.
    assert_eq!(top_file(&mut shell, &tab), 2);
}

#[gpui_kit::test]
fn viewed_progress_is_compact_with_a_tooltip(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(progress(&mut shell, &tab), (0, 3));
    assert!(shows(shell.cx, "viewed-progress-label", "0/3"));
    // Compact: the count alone, no bar (v1's bar alone was 48 pt).
    let width = bounds(shell.cx, "viewed-progress").size.width;
    assert!(width < px(48.), "{width:?}");
    // The tooltip says it in words.
    hover(shell.cx, "viewed-progress");
    shell.cx.executor().advance_clock(Duration::from_secs(2));
    draw(shell.cx);
    assert!(painted(shell.cx, "tooltip: 0 of 3 files viewed").is_some());
    unhover(shell.cx);

    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "v");
    assert_eq!(progress(&mut shell, &tab), (1, 3));
    assert!(shows(shell.cx, "viewed-progress-label", "1/3"));
    header_checkbox(&mut shell, &tab, 1);
    header_checkbox(&mut shell, &tab, 2);
    assert_eq!(progress(&mut shell, &tab), (3, 3));
    assert!(shows(shell.cx, "viewed-progress-label", "3/3"));
    header_checkbox(&mut shell, &tab, 1);
    assert_eq!(progress(&mut shell, &tab), (2, 3));
}

#[gpui_kit::test]
fn viewed_toggle_never_pins_live_state(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    repo.git(&["checkout", "-q", "refs/tags/head"]);
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.write("src/new.rs", b"pub fn new() {}\n");
    let mut shell = start(cx);
    let req = OpenRequest {
        source: Source::Live { since: Since::Head },
        ..compare_req(repo.path())
    };
    let tab = shell.open(req).unwrap();
    let review_id = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let iterations = shell.core.iterations(&review_id).unwrap().len();
    let refs = repo.git(&["for-each-ref", "refs/polygloss/"]);
    let files = tab.read_with(shell.cx, |t, _| t.opened.files.len());
    assert_eq!(files, 2, "two uncommitted changes");

    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "v");
    header_checkbox(&mut shell, &tab, 1);
    assert_eq!(stored(&mut shell, &tab), [Viewed, Viewed]);
    assert_eq!(shell.core.iterations(&review_id).unwrap().len(), iterations);
    assert_eq!(
        repo.git(&["for-each-ref", "refs/polygloss/"]),
        refs,
        "no pin"
    );

    // With the cursor in a file, `v` toggles that file, wherever the
    // viewport is scrolled.
    header_checkbox(&mut shell, &tab, 0);
    tab.update(shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| {
            v.set_cursor(
                Some(polygloss_viewport::CursorPos {
                    file_idx: 0,
                    side: Side::New,
                    line: 0,
                    range_start: None,
                }),
                cx,
            )
        })
    });
    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "v");
    assert_eq!(stored(&mut shell, &tab), [Viewed, Viewed]);
    assert_eq!(shell.core.iterations(&review_id).unwrap().len(), iterations);
    assert_eq!(
        repo.git(&["for-each-ref", "refs/polygloss/"]),
        refs,
        "no pin"
    );
}

// ---------------------------------------------------------------------------
// Category sections (T6.14, design §9): Mark all viewed, progress over every
// file, and the jump after `v` walking display order past closed sections.

use crate::categories::{act, click_band, repo_with, section_id, sections, set_cursor};
use polygloss_app::keymap::actions::categories as category_actions;

/// In diff order: 0 `src/a.rs`, 1 `src/a.test.rs`, 2 `src/b.rs`, 3
/// `tests/it.rs`: two main files and two test files, `lines` long each
/// (short ones leave the Tests band in view).
fn two_and_two(lines: usize) -> FixtureRepo {
    repo_with(
        &["src/a.rs", "src/a.test.rs", "src/b.rs", "tests/it.rs"],
        lines,
    )
}

fn anchor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> polygloss_viewport::ScrollAnchor {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor())
}

#[gpui_kit::test]
fn mark_all_viewed_marks_every_file_of_the_section(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = two_and_two(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    set_cursor(&mut shell, &tab, 0, 10);
    let (cursor, at) = (cursor_file(&mut shell, &tab), anchor(&mut shell, &tab));
    let tests = section_id(&mut shell, &tab, "tests");
    let band = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            t.viewport
                .read(cx)
                .debug()
                .bands
                .into_iter()
                .next()
                .expect("the Tests band is painted")
                .links
        })
    };
    assert_eq!(band(&mut shell), ["Show", "Mark all viewed"]);

    click_band(
        &mut shell,
        &tab,
        polygloss_viewport::ControlAction::SectionMarkViewed(tests),
    );
    assert_eq!(
        stored(&mut shell, &tab),
        [NotViewed, Viewed, NotViewed, Viewed]
    );
    // The section stays closed and the view does not move.
    assert!(!sections(&mut shell, &tab)[0].2, "Tests stays closed");
    assert_eq!(cursor_file(&mut shell, &tab), cursor);
    assert_eq!(anchor(&mut shell, &tab), at);
    assert_eq!(band(&mut shell), ["Show", "Mark all unviewed"]);
}

#[gpui_kit::test]
fn mark_all_viewed_on_an_open_section_holding_the_cursor_jumps_from_its_last_file(
    cx: &mut TestAppContext,
) {
    let _sb = Sandbox::isolate();
    let repo = two_and_two(80);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // `src/a.rs` viewed, so the jump has to wrap past it to `src/b.rs`.
    header_checkbox(&mut shell, &tab, 0);
    set_cursor(&mut shell, &tab, 1, 5);
    assert!(sections(&mut shell, &tab)[0].2, "the cursor opened it");

    act(&mut shell, &tab, category_actions::MarkSectionViewed);
    assert_eq!(
        stored(&mut shell, &tab),
        [Viewed, Viewed, NotViewed, Viewed]
    );
    assert!(collapsed(&mut shell, &tab).contains(&1));
    assert!(collapsed(&mut shell, &tab).contains(&3));
    // From `tests/it.rs`, the last in display order: nothing after it, so it
    // wraps to the first unviewed shown file.
    assert_eq!(cursor_file(&mut shell, &tab), Some(2));
    assert_eq!(top_file(&mut shell, &tab), 2);
}

#[gpui_kit::test]
fn viewed_progress_counts_categorized_files(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = two_and_two(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert!(shows(shell.cx, "viewed-progress-label", "0/4"));
    let tests = section_id(&mut shell, &tab, "tests");
    click_band(
        &mut shell,
        &tab,
        polygloss_viewport::ControlAction::SectionMarkViewed(tests),
    );
    assert!(shows(shell.cx, "viewed-progress-label", "2/4"));
}

#[gpui_kit::test]
fn marking_viewed_jumps_in_display_order_and_skips_closed_sections(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // 0 `src/a.rs`, 1 `src/a.test.rs` (Tests, closed), 2 `src/b.rs`.
    let repo = repo_with(&["src/a.rs", "src/a.test.rs", "src/b.rs"], 80);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    focus_viewport(&mut shell, &tab);
    set_cursor(&mut shell, &tab, 0, 3);
    keys(&mut shell, "v");
    assert_eq!(cursor_file(&mut shell, &tab), Some(2));
    assert_eq!(top_file(&mut shell, &tab), 2);

    // `v` on `src/b.rs`: `src/a.rs` is viewed and `src/a.test.rs` hidden, so
    // there is nowhere to go.
    keys(&mut shell, "v");
    assert_eq!(stored(&mut shell, &tab), [Viewed, NotViewed, Viewed]);
    assert_eq!(top_file(&mut shell, &tab), 2);
    assert!(!sections(&mut shell, &tab)[0].2, "Tests stays closed");

    // With `src/a.rs` unviewed again, `v` on `src/b.rs` wraps to it.
    header_checkbox(&mut shell, &tab, 0);
    header_checkbox(&mut shell, &tab, 2);
    focus_viewport(&mut shell, &tab);
    set_cursor(&mut shell, &tab, 2, 3);
    keys(&mut shell, "v");
    assert_eq!(cursor_file(&mut shell, &tab), Some(0));
    assert_eq!(top_file(&mut shell, &tab), 0);
}

#[gpui_kit::test]
fn marking_a_hidden_file_viewed_never_moves_the_view(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = two_and_two(80);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    set_cursor(&mut shell, &tab, 0, 10);
    let (cursor, at) = (cursor_file(&mut shell, &tab), anchor(&mut shell, &tab));

    // The tree's row of a test file (its section closed).
    let tree = tree_of(&mut shell, &tab);
    tree.update(shell.cx, |_, cx| {
        cx.emit(polygloss_app::tree::FileTreeEvent::ToggleViewed(1))
    });
    draw(shell.cx);
    assert_eq!(stored(&mut shell, &tab)[1], Viewed);
    assert_eq!(cursor_file(&mut shell, &tab), cursor);
    assert_eq!(anchor(&mut shell, &tab), at);
    assert!(!sections(&mut shell, &tab)[0].2, "Tests stays closed");

    // Its folder: hidden files only, so no jump either.
    tree.update(shell.cx, |_, cx| {
        cx.emit(polygloss_app::tree::FileTreeEvent::ToggleFolderViewed {
            dir: "tests".into(),
            files: vec![3],
        })
    });
    draw(shell.cx);
    assert_eq!(stored(&mut shell, &tab)[3], Viewed);
    assert_eq!(anchor(&mut shell, &tab), at);
}
