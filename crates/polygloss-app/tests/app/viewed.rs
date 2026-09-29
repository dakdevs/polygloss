//! GPUI tests of T3.7: Viewed UX (design §9, ADR-0022). `v` in the viewport
//! and the tree, the header and tree checkboxes, folder tri-state and "Mark
//! folder viewed", the toolbar's "N / M viewed", carry-over across reopening
//! and iterations, the "changed since viewed" badge, and never pinning.

use gpui_kit::{Entity, TestAppContext};
use polygloss_app::keymap::actions::tree as tree_actions;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::tree::row::Check;
use polygloss_app::tree::{FileTree, file_tree};
use polygloss_app::viewed;
use polygloss_core::git::{Since, Source};
use polygloss_core::review::{OpenRequest, ViewedState};
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{FileFlags, ViewportEvent};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

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
fn viewed_progress_in_toolbar(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(progress(&mut shell, &tab), (0, 3));
    let label = tab.read_with(shell.cx, |t, _| viewed::progress_label(t));
    assert_eq!(label, "0 / 3 viewed");
    assert!(shell.cx.debug_bounds("viewed-progress").is_some());

    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "v");
    assert_eq!(progress(&mut shell, &tab), (1, 3));
    let label = tab.read_with(shell.cx, |t, _| viewed::progress_label(t));
    assert_eq!(label, "1 / 3 viewed");
    header_checkbox(&mut shell, &tab, 1);
    header_checkbox(&mut shell, &tab, 2);
    assert_eq!(progress(&mut shell, &tab), (3, 3));
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
