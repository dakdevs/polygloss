//! The Files segment's accordion (T6.15, design §11.5, OQ-44): Changes,
//! then one panel per non-empty enabled category, one open at a time,
//! each scrolling on its own; its headers' counts and indicators; the open
//! panel following the viewport; one filter over every panel; folders
//! covering their own panel's files; and the tree keys walking the open
//! panel.
//!
//! Expected panels are written by hand from design §11.15's table (Tests
//! takes `*.test.*` and `tests/`, Generated lockfiles, Docs is off).

use gpui_kit::{Entity, ScrollStrategy, TestAppContext};
use polygloss_app::keymap::actions::tree as tree_actions;
use polygloss_app::live;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::tree::row::Check;
use polygloss_app::tree::{FileTree, file_tree};
use polygloss_app::viewed;
use polygloss_viewport::FileFlags;

use crate::categories::{
    cursor, go_to_file, mixed_repo, numstat, repo_with, section_id, set_cursor,
};
use crate::shell::{Shell, compare_req, draw, painted, start};
use crate::support::{FixtureRepo, Sandbox};
use crate::toolbar::shows;

struct Opened<'a> {
    shell: Shell<'a>,
    tab: Entity<ReviewTab>,
    tree: Entity<FileTree>,
}

fn open<'a>(cx: &'a mut TestAppContext, repo: &FixtureRepo) -> Opened<'a> {
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    let tree = tab.read_with(shell.cx, |t, _| file_tree(t).cloned().expect("a file tree"));
    Opened { shell, tab, tree }
}

/// `(panel, files)` of every panel, top to bottom.
fn panels(o: &mut Opened) -> Vec<(String, Vec<u32>)> {
    o.tree.read_with(o.shell.cx, |t, _| {
        t.panels()
            .panels()
            .iter()
            .map(|p| (p.key().to_string(), p.files().to_vec()))
            .collect()
    })
}

fn p(key: &str, files: &[u32]) -> (String, Vec<u32>) {
    (key.to_owned(), files.to_vec())
}

fn open_panel(o: &mut Opened) -> Option<String> {
    o.tree.read_with(o.shell.cx, |t, _| {
        t.panels().open_key().map(ToString::to_string)
    })
}

/// The labels of the open panel's rows.
fn rows(o: &mut Opened) -> Vec<String> {
    o.tree.read_with(o.shell.cx, |t, cx| {
        t.rows(cx).into_iter().map(|r| r.label).collect()
    })
}

fn click(o: &mut Opened, selector: &str) {
    crate::shell::click(o.shell.cx, selector);
}

fn header(o: &mut Opened, key: &str) -> bool {
    painted(o.shell.cx, &format!("tree-panel-{key}")).is_some()
}

fn top_file(o: &mut Opened) -> u32 {
    o.tab
        .read_with(o.shell.cx, |t, cx| t.viewport.read(cx).anchor().file_idx)
}

fn selected_file(o: &mut Opened) -> Option<u32> {
    o.tree.read_with(o.shell.cx, |t, _| t.selected_file())
}

fn section_open(o: &mut Opened, category: &str) -> Option<bool> {
    let id = section_id(&mut o.shell, &o.tab, category);
    o.tab
        .read_with(o.shell.cx, |t, cx| t.viewport.read(cx).section_open(id))
}

fn viewed_files(o: &mut Opened) -> Vec<u32> {
    o.tree.read_with(o.shell.cx, |t, _| {
        t.file_flags()
            .iter()
            .enumerate()
            .filter(|(_, f)| f.viewed)
            .map(|(i, _)| i as u32)
            .collect()
    })
}

fn set_query(o: &mut Opened, query: &str) {
    let tree = o.tree.clone();
    let query = query.to_owned();
    o.shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.set_query(&query, window, cx)));
    draw(o.shell.cx);
}

fn scroll_by(o: &mut Opened, dy: f32) {
    let viewport = o.tab.read_with(o.shell.cx, |t, _| t.viewport.clone());
    viewport.update(o.shell.cx, |v, cx| v.scroll_by(dy, cx));
    draw(o.shell.cx);
}

/// Pushes the file flags `f` sets, as the threads and Viewed features do.
fn push_flags(o: &mut Opened, f: impl FnOnce(&mut Vec<FileFlags>)) {
    o.tab
        .update(o.shell.cx, |t, cx| viewed::update_file_flags(t, cx, f));
    draw(o.shell.cx);
}

/// In diff order: 0 `Cargo.lock`, 1 `README.md`, 2 `src/config.rs`, 3
/// `src/config.test.ts`, 4 `src/main.rs`, 5 `src/main.test.ts`, 6
/// `tests/config.rs`, 7 `tests/util.rs`: Changes 1, 2, 4; Tests 3, 5, 6, 7;
/// Generated 0.
fn config_repo() -> FixtureRepo {
    repo_with(
        &[
            "Cargo.lock",
            "README.md",
            "src/config.rs",
            "src/config.test.ts",
            "src/main.rs",
            "src/main.test.ts",
            "tests/config.rs",
            "tests/util.rs",
        ],
        20,
    )
}

/// In diff order: 0 `Cargo.lock`, 1 `docs/x.md`, 2 `src/a.rs`, 3
/// `src/a.test.rs`, 4 `src/b.rs`, 5 `src/b.test.rs`, 6 `src/c.rs`, 7
/// `tests/it.rs`: Changes 1, 2, 4, 6; Tests 3, 5, 7; Generated 0.
fn three_tests_repo() -> FixtureRepo {
    repo_with(
        &[
            "Cargo.lock",
            "docs/x.md",
            "src/a.rs",
            "src/a.test.rs",
            "src/b.rs",
            "src/b.test.rs",
            "src/c.rs",
            "tests/it.rs",
        ],
        30,
    )
}

#[gpui_kit::test]
fn accordion_has_changes_then_a_panel_per_nonempty_category(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    // `docs/x.md` stays in Changes (Docs is off); Vendored and the other
    // categories have no files, so no panel.
    assert_eq!(
        panels(&mut o),
        [
            p("changes", &[1, 2, 4]),
            p("tests", &[3, 5]),
            p("generated", &[0])
        ]
    );
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    for (key, title, count) in [
        ("changes", "Changes", "3"),
        ("tests", "Tests", "2"),
        ("generated", "Generated", "1"),
    ] {
        assert!(shows(o.shell.cx, &format!("tree-panel-title-{key}"), title));
        assert!(shows(o.shell.cx, &format!("tree-panel-count-{key}"), count));
    }
    // Top to bottom: Changes' header, its tree, then the closed panels'
    // headers.
    let top = |o: &mut Opened, name: &str| crate::shell::bounds(o.shell.cx, name).top();
    let last_row = crate::shell::bounds(o.shell.cx, "tree-row-f:4");
    assert!(top(&mut o, "tree-panel-changes") < last_row.top());
    assert!(last_row.bottom() <= top(&mut o, "tree-panel-tests"));
    assert!(top(&mut o, "tree-panel-tests") < top(&mut o, "tree-panel-generated"));
    // The open panel's rows: Changes' files only.
    assert_eq!(rows(&mut o), ["docs", "x.md", "src", "a.rs", "b.rs"]);
    assert!(painted(o.shell.cx, "tree-row-f:3").is_none());
}

#[gpui_kit::test]
fn every_file_categorized_leaves_out_the_changes_panel(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = repo_with(&["Cargo.lock", "src/a.test.rs", "tests/it.rs"], 20);
    let mut o = open(cx, &repo);
    assert_eq!(panels(&mut o), [p("tests", &[1, 2]), p("generated", &[0])]);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(!header(&mut o, "changes"));
    assert!(header(&mut o, "tests") && header(&mut o, "generated"));
    assert_eq!(rows(&mut o), ["src", "a.test.rs", "tests", "it.rs"]);
}

#[gpui_kit::test]
fn one_panel_open_at_a_time_each_scrolls(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // 0–29 `src/m00.rs`…, 30–59 `tests/t00.rs`…: each panel taller than
    // the sidebar.
    let paths: Vec<String> = (0..30)
        .map(|i| format!("src/m{i:02}.rs"))
        .chain((0..30).map(|i| format!("tests/t{i:02}.rs")))
        .collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    let repo = repo_with(&paths, 3);
    let mut o = open(cx, &repo);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    assert!(painted(o.shell.cx, "tree-row-f:0").is_some());

    let tree_focused = |o: &mut Opened| {
        let tree = o.tree.clone();
        o.shell
            .cx
            .update(|window, cx| tree.read(cx).contains_focus(window, cx))
    };
    assert!(!tree_focused(&mut o), "the diff has the keyboard");
    click(&mut o, "tree-panel-tests");
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(painted(o.shell.cx, "tree-row-f:30").is_some());
    assert!(
        painted(o.shell.cx, "tree-row-f:0").is_none(),
        "Changes closed"
    );
    // The header gave the opened panel's list the keyboard: `n` walks it.
    assert!(tree_focused(&mut o));
    o.shell.cx.simulate_keystrokes("n");
    draw(o.shell.cx);
    assert_eq!(selected_file(&mut o), Some(30));

    // Each panel is its own list with its own scroll position.
    let (changes, tests) = o.tree.read_with(o.shell.cx, |t, _| {
        let state = |key: &str| {
            t.panels()
                .panels()
                .iter()
                .find(|p| p.key().to_string() == key)
                .expect("the panel")
                .tree_state()
                .clone()
        };
        (state("changes"), state("tests"))
    });
    assert_ne!(changes.entity_id(), tests.entity_id());
    let offset = |o: &mut Opened, state: &Entity<gpui_kit::component::tree::TreeState>| {
        state.read_with(o.shell.cx, |s, _| {
            s.scroll_handle().0.borrow().base_handle.offset().y
        })
    };
    tests.update(o.shell.cx, |s, _| s.scroll_to_item(28, ScrollStrategy::Top));
    draw(o.shell.cx);
    let scrolled = offset(&mut o, &tests);
    assert!(scrolled < gpui_kit::px(0.), "the Tests list scrolled");
    click(&mut o, "tree-panel-changes");
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    assert_eq!(offset(&mut o, &changes), gpui_kit::px(0.));
    click(&mut o, "tree-panel-tests");
    assert_eq!(offset(&mut o, &tests), scrolled, "Tests kept its place");
}

#[gpui_kit::test]
fn panel_header_shows_open_threads_agent_and_changed(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(20);
    let mut o = open(cx, &repo);
    push_flags(&mut o, |f| {
        f[3].open_threads = 2;
        f[3].agent_threads = true;
        f[5].open_threads = 1;
        f[5].changed_since_viewed = true;
        f[2].open_threads = 1;
    });
    // Tests: 2 + 1 open threads, an agent's, and a file changed since
    // viewed; Changes: one open thread; Generated: nothing.
    assert!(shows(o.shell.cx, "tree-panel-threads-tests", "3"));
    assert!(header(&mut o, "agent-tests"));
    assert!(header(&mut o, "changed-tests"));
    assert!(shows(o.shell.cx, "tree-panel-threads-changes", "1"));
    assert!(!header(&mut o, "agent-changes"));
    assert!(!header(&mut o, "changed-changes"));
    for part in ["threads", "agent", "changed"] {
        assert!(!header(&mut o, &format!("{part}-generated")), "{part}");
    }
}

#[gpui_kit::test]
fn selecting_a_category_file_opens_its_section(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    let before = crate::categories::anchor(&mut o.shell, &o.tab);
    // Opening a panel moves nothing in the diff.
    click(&mut o, "tree-panel-tests");
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert_eq!(section_open(&mut o, "tests"), Some(false));
    assert_eq!(crate::categories::anchor(&mut o.shell, &o.tab), before);
    // Choosing a row of it opens the section and goes there.
    click(&mut o, "tree-row-f:5");
    assert_eq!(section_open(&mut o, "tests"), Some(true));
    assert_eq!(top_file(&mut o), 5);
    assert_eq!(selected_file(&mut o), Some(5));
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
}

#[gpui_kit::test]
fn panel_follows_jumps_and_section_crossings(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    // A jump into a closed section (an explicit target): it opens, and so
    // does its panel, with the file's row marked.
    go_to_file(&mut o.shell, &o.tab, 3);
    assert_eq!(section_open(&mut o, "tests"), Some(true));
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert_eq!(selected_file(&mut o), Some(3));
    // Scrolling up out of the section into `src/b.rs`: Changes opens.
    scroll_by(&mut o, -200.0);
    assert_eq!(top_file(&mut o), 4);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    assert_eq!(selected_file(&mut o), Some(4));
    // And down again into the section: Tests.
    scroll_by(&mut o, 400.0);
    assert_eq!(top_file(&mut o), 3);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
}

#[gpui_kit::test]
fn a_user_opened_panel_stays_until_the_next_crossing(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    go_to_file(&mut o.shell, &o.tab, 2);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    click(&mut o, "tree-panel-generated");
    assert_eq!(open_panel(&mut o).as_deref(), Some("generated"));
    // Scrolling within the main files crosses no section.
    go_to_file(&mut o.shell, &o.tab, 4);
    assert_eq!(top_file(&mut o), 4);
    assert_eq!(open_panel(&mut o).as_deref(), Some("generated"));
    // Into Tests: its panel opens.
    go_to_file(&mut o.shell, &o.tab, 5);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
}

#[gpui_kit::test]
fn filter_applies_to_every_panel_and_shows_match_counts(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = config_repo();
    let mut o = open(cx, &repo);
    assert_eq!(
        panels(&mut o),
        [
            p("changes", &[1, 2, 4]),
            p("tests", &[3, 5, 6, 7]),
            p("generated", &[0])
        ]
    );
    set_query(&mut o, "config");
    assert!(shows(o.shell.cx, "tree-panel-count-changes", "1 of 3"));
    assert!(shows(o.shell.cx, "tree-panel-count-tests", "2 of 4"));
    let matches = o.tree.read_with(o.shell.cx, |t, _| {
        t.panels()
            .panels()
            .iter()
            .map(|p| p.matches())
            .collect::<Vec<_>>()
    });
    assert_eq!(matches, [(1, 3), (2, 4), (0, 1)]);
    // Changes keeps the keyboard's place; each panel shows its matches.
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    assert_eq!(rows(&mut o), ["src", "config.rs"]);
    click(&mut o, "tree-panel-tests");
    assert_eq!(
        rows(&mut o),
        ["src", "config.test.ts", "tests", "config.rs"]
    );
    // The footer ignores the filter: every Changes file, from git.
    super::settle_counts(&mut o.shell, &o.tab, 8);
    let changes = ["README.md", "src/config.rs", "src/main.rs"];
    let (files, added, removed) = numstat(&repo, |p| changes.contains(&p));
    assert_eq!(files, 3);
    assert!(shows(
        o.shell.cx,
        "tree-footer",
        &format!("Total: +{added} −{removed}")
    ));
}

#[gpui_kit::test]
fn filter_hides_panels_without_matches(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = config_repo();
    let mut o = open(cx, &repo);
    set_query(&mut o, "config");
    assert!(header(&mut o, "changes") && header(&mut o, "tests"));
    assert!(!header(&mut o, "generated"), "no match in Generated");
    set_query(&mut o, "zzz");
    for key in ["changes", "tests", "generated"] {
        assert!(!header(&mut o, key), "{key}");
    }
    assert!(painted(o.shell.cx, "tree-clear-filters").is_some());
    click(&mut o, "tree-clear-filters");
    for key in ["changes", "tests", "generated"] {
        assert!(header(&mut o, key), "{key}");
    }
}

/// Changes open; "has comments" with open threads only on
/// `tests/config.rs`.
fn has_comments_in_tests_only(o: &mut Opened) {
    push_flags(o, |f| f[6].open_threads = 1);
    assert_eq!(open_panel(o).as_deref(), Some("changes"));
    o.tree.update(o.shell.cx, |t, cx| t.toggle_has_comments(cx));
    draw(o.shell.cx);
}

#[gpui_kit::test]
fn filter_matching_only_a_category_opens_its_panel(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = config_repo();
    let mut o = open(cx, &repo);
    has_comments_in_tests_only(&mut o);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(!header(&mut o, "changes"));
    assert!(shows(o.shell.cx, "tree-panel-count-tests", "1 of 4"));
    assert_eq!(rows(&mut o), ["tests", "config.rs"]);
}

#[gpui_kit::test]
fn clearing_the_filter_keeps_the_open_panel(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = config_repo();
    let mut o = open(cx, &repo);
    has_comments_in_tests_only(&mut o);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    let tree = o.tree.clone();
    o.shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.clear_filters(window, cx)));
    draw(o.shell.cx);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(shows(o.shell.cx, "tree-panel-count-changes", "3"));
    assert!(shows(o.shell.cx, "tree-panel-count-tests", "4"));
}

#[gpui_kit::test]
fn folder_viewed_covers_only_its_panels_files(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(20);
    let mut o = open(cx, &repo);
    // `src/` in Changes holds `src/a.rs` and `src/b.rs`, not `src/a.test.rs`.
    click(&mut o, "tree-check-d:src");
    assert_eq!(viewed_files(&mut o), [2, 4]);
    let check = |o: &mut Opened, key: &str| {
        o.tree.read_with(o.shell.cx, |t, _| {
            t.panels()
                .panels()
                .iter()
                .find(|p| p.key().to_string() == key)
                .and_then(|p| p.folder_check("src"))
        })
    };
    assert_eq!(check(&mut o, "changes"), Some(Check::On));
    // The Tests panel's `src/`: nothing viewed, so its slot is empty at rest.
    assert_eq!(check(&mut o, "tests"), Some(Check::Off));
    click(&mut o, "tree-panel-tests");
    for glyph in ["circle", "circle-check", "circle-minus"] {
        assert!(
            painted(o.shell.cx, &format!("tree-slot-d:src: {glyph}")).is_none(),
            "{glyph}"
        );
    }
}

#[gpui_kit::test]
fn panels_rebuild_once_per_refresh(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(20);
    let mut o = open(cx, &repo);
    let builds = |o: &mut Opened| o.tree.read_with(o.shell.cx, |t, _| t.panel_builds());
    let before = builds(&mut o);
    assert_eq!(before, 1, "built once, at open");
    // `head` moves to a commit with one more test file.
    repo.write("src/c.test.rs", b"c\n");
    repo.commit("more");
    repo.git(&["tag", "-f", "head"]);
    o.tab.update_in(o.shell.cx, live::refresh_tab);
    draw(o.shell.cx);
    assert_eq!(builds(&mut o), before + 1);
    // In diff order the new file is 5: Changes 1, 2, 4; Tests 3, 5, 6.
    assert_eq!(
        panels(&mut o),
        [
            p("changes", &[1, 2, 4]),
            p("tests", &[3, 5, 6]),
            p("generated", &[0])
        ]
    );
}

#[gpui_kit::test]
fn marking_a_hidden_file_viewed_from_its_panel_never_moves_the_view(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    set_cursor(&mut o.shell, &o.tab, 2, 5);
    let anchor = crate::categories::anchor(&mut o.shell, &o.tab);
    let at = cursor(&mut o.shell, &o.tab);
    click(&mut o, "tree-panel-tests");
    click(&mut o, "tree-check-f:3");
    assert_eq!(viewed_files(&mut o), [3]);
    assert_eq!(section_open(&mut o, "tests"), Some(false));
    assert_eq!(crate::categories::anchor(&mut o.shell, &o.tab), anchor);
    assert_eq!(cursor(&mut o.shell, &o.tab), at);
}

#[gpui_kit::test]
fn mark_folder_viewed_jumps_from_its_last_file_in_display_order(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = three_tests_repo();
    let mut o = open(cx, &repo);
    go_to_file(&mut o.shell, &o.tab, 3);
    set_cursor(&mut o.shell, &o.tab, 3, 0);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    // `src/` of the Tests panel: `src/a.test.rs` and `src/b.test.rs`.
    click(&mut o, "tree-row-d:src");
    let dir = o.tree.read_with(o.shell.cx, |t, _| t.selected_dir());
    assert_eq!(dir, Some(("src".to_owned(), vec![3, 5])));
    o.shell.cx.dispatch_action(tree_actions::MarkFolderViewed);
    draw(o.shell.cx);
    assert_eq!(viewed_files(&mut o), [3, 5]);
    // The next unviewed file after `src/b.test.rs` in display order is
    // `tests/it.rs` (a walk in git order would reach `src/c.rs`).
    assert_eq!(cursor(&mut o.shell, &o.tab).map(|c| c.file_idx), Some(7));
    assert_eq!(top_file(&mut o), 7);
}

#[gpui_kit::test]
fn tree_n_p_walk_the_open_panel(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = three_tests_repo();
    let mut o = open(cx, &repo);
    click(&mut o, "tree-panel-tests");
    click(&mut o, "tree-row-f:3");
    assert_eq!(selected_file(&mut o), Some(3));
    let key = |o: &mut Opened, k: &str| {
        o.shell.cx.simulate_keystrokes(k);
        draw(o.shell.cx);
        selected_file(o)
    };
    assert_eq!(key(&mut o, "n"), Some(5));
    assert_eq!(key(&mut o, "n"), Some(7));
    // Wraps within the panel, never into Changes.
    assert_eq!(key(&mut o, "n"), Some(3));
    assert_eq!(key(&mut o, "p"), Some(7));
    assert_eq!(top_file(&mut o), 7);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
}
