//! The Files segment's accordion (T6.15, design §11.5, OQ-44): Changes,
//! then one panel per non-empty enabled category, one open at a time,
//! each scrolling on its own; its headers' counts and indicators; the open
//! panel following the viewport; one filter over every panel; folders
//! covering their own panel's files; and the tree keys walking the open
//! panel. Then M9, the panel switch's motion (T7.13, ADR-0030).
//!
//! Expected panels are written by hand from design §11.15's table (Tests
//! takes `*.test.*` and `tests/`, Generated lockfiles, Docs is off).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    Entity, Modifiers, MouseButton, ScrollDelta, ScrollStrategy, ScrollWheelEvent, TestAppContext,
    point, px,
};
use polygloss_app::keymap::actions::tree as tree_actions;
use polygloss_app::live;
use polygloss_app::motion::tokens::FIRST_STEP;
use polygloss_app::motion::{self, Initiator, MotionPolicy, Recorded};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::tree::row::Check;
use polygloss_app::tree::{FileTree, file_tree};
use polygloss_app::viewed;
use polygloss_viewport::FileFlags;

use crate::categories::{
    cursor, go_to_file, mixed_repo, numstat, reload_settings, repo_with, scroll_to_line,
    section_id, set_cursor,
};
use crate::shell::{Shell, compare_req, draw, painted, start};
use crate::support::motion::{advance, frame, requested_frames, step_to};
use crate::support::{FixtureRepo, Sandbox};
use crate::toolbar::shows;

struct Opened<'a> {
    shell: Shell<'a>,
    tab: Entity<ReviewTab>,
    tree: Entity<FileTree>,
}

/// `repo` opened with app motion off: these tests act on a header click's
/// result at once (M9's own tests turn Full on).
fn open<'a>(cx: &'a mut TestAppContext, repo: &FixtureRepo) -> Opened<'a> {
    cx.update(|cx| motion::set_override(Some(MotionPolicy::Off), cx));
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
fn a_repartition_moving_the_top_file_into_a_panel_opens_it(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut o = open(cx, &repo);
    // Reading `docs/x.md` (Changes) with Generated opened by hand.
    scroll_to_line(&mut o.shell, &o.tab, 1, 60);
    click(&mut o, "tree-panel-generated");
    assert_eq!(open_panel(&mut o).as_deref(), Some("generated"));
    // Docs goes on: the view lands on its band, a crossing into Docs.
    reload_settings(
        &mut o.shell,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    assert_eq!(top_file(&mut o), 1);
    assert_eq!(open_panel(&mut o).as_deref(), Some("docs"));
    assert_eq!(selected_file(&mut o), Some(1));
    // A repartition that leaves the top file in its panel keeps the one
    // the user opened.
    click(&mut o, "tree-panel-generated");
    reload_settings(
        &mut o.shell,
        r#"{ "categories": { "docs": { "enabled": true }, "tests": { "enabled": false } } }"#,
    );
    assert_eq!(
        panels(&mut o),
        [
            p("changes", &[2, 3, 4, 5]),
            p("generated", &[0]),
            p("docs", &[1])
        ]
    );
    assert_eq!(open_panel(&mut o).as_deref(), Some("generated"));
}

#[gpui_kit::test]
fn a_crossing_never_carries_a_selected_folder_into_the_next_panel(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut shell = start(cx);
    // Tests off: Changes holds `src/a.rs`, `src/a.test.rs`, `src/b.rs`.
    reload_settings(
        &mut shell,
        r#"{ "categories": { "tests": { "enabled": false } } }"#,
    );
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    let tree = tab.read_with(shell.cx, |t, _| file_tree(t).cloned().unwrap());
    let mut o = Opened { shell, tab, tree };
    scroll_to_line(&mut o.shell, &o.tab, 3, 60);
    click(&mut o, "tree-row-d:src");
    let dir = o.tree.read_with(o.shell.cx, |t, _| t.selected_dir());
    assert_eq!(dir.map(|(d, _)| d).as_deref(), Some("src"));
    // Tests on: `src/a.test.rs` moves into it, and so does the view (onto
    // its band). Tests' own `src/` is another folder: the row marked is
    // the top file's.
    reload_settings(&mut o.shell, "{}");
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert_eq!(selected_file(&mut o), Some(3));
}

#[gpui_kit::test]
fn following_a_crossing_keeps_the_keyboard_in_the_tree(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    // Tests' section open, the view and cursor on `src/b.rs` (Changes).
    go_to_file(&mut o.shell, &o.tab, 3);
    go_to_file(&mut o.shell, &o.tab, 4);
    set_cursor(&mut o.shell, &o.tab, 4, 0);
    let tree = o.tree.clone();
    o.shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.focus(window, cx)));
    draw(o.shell.cx);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    assert_eq!(selected_file(&mut o), Some(4));
    // `v` in the tree: the jump to the next unviewed file crosses into
    // Tests; its list takes the keyboard from the Changes list.
    o.shell.cx.simulate_keystrokes("v");
    draw(o.shell.cx);
    assert_eq!(viewed_files(&mut o), [4]);
    assert_eq!(top_file(&mut o), 3);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    let focused = o
        .shell
        .cx
        .update(|window, cx| tree.read(cx).contains_focus(window, cx));
    assert!(focused);
    o.shell.cx.simulate_keystrokes("n");
    draw(o.shell.cx);
    assert_eq!(selected_file(&mut o), Some(5));
}

#[gpui_kit::test]
fn a_filter_opening_another_panel_leaves_the_keyboard_in_the_filter_box(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = config_repo();
    let mut o = open(cx, &repo);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    let tree = o.tree.clone();
    o.shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.focus_filter(window, cx)));
    draw(o.shell.cx);
    let filter = o.tree.read_with(o.shell.cx, |t, cx| t.filter_focus(cx));
    let in_box = |o: &mut Opened| o.shell.cx.update(|window, _| filter.is_focused(window));
    assert!(in_box(&mut o));
    // No Changes or Generated path holds a `t`: the first letter typed
    // leaves only Tests matching, and its panel opens.
    o.shell.cx.simulate_input("t");
    draw(o.shell.cx);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(in_box(&mut o), "the panel switch took the keyboard");
    o.shell.cx.simulate_input("est");
    draw(o.shell.cx);
    let query = o
        .tree
        .read_with(o.shell.cx, |t, _| t.filters().query.clone());
    assert_eq!(query, "test");
}

#[gpui_kit::test]
fn file_finder_lists_files_in_display_order(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // Git order: 0 `Cargo.lock` (Generated), 1 `tests/x.rs` (Tests), 2
    // `units/x.rs` (Changes); display order 2, 1, 0.
    let repo = repo_with(&["Cargo.lock", "tests/x.rs", "units/x.rs"], 3);
    let mut o = open(cx, &repo);
    o.shell.cx.simulate_keystrokes("cmd-p");
    draw(o.shell.cx);
    let finder = o
        .shell
        .cx
        .update(|_, cx| polygloss_app::tree::finder::current(cx))
        .expect("the finder is open");
    let listed =
        |o: &mut Opened| finder.read_with(o.shell.cx, |s, _| s.delegate().matches().to_vec());
    assert_eq!(listed(&mut o), [2, 1, 0]);
    // `x.rs` matches both paths at the same place after a five-letter
    // folder: equal scores, so display order breaks the tie.
    o.shell.cx.simulate_input("x.rs");
    draw(o.shell.cx);
    assert_eq!(listed(&mut o), [2, 1]);
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
fn a_refresh_keeps_a_user_opened_panel_while_the_top_file_stays_in_its_panel(
    cx: &mut TestAppContext,
) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let mut o = open(cx, &repo);
    // Reading `src/b.rs` (Changes) with Tests opened by hand.
    go_to_file(&mut o.shell, &o.tab, 4);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    click(&mut o, "tree-panel-tests");
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    let refresh = |o: &mut Opened, path: &str| {
        repo.write(path, b"new\n");
        repo.commit(path);
        repo.git(&["tag", "-f", "head"]);
        o.tab.update_in(o.shell.cx, live::refresh_tab);
        draw(o.shell.cx);
    };
    // A file after `src/b.rs`: it stays file 4 of Changes.
    refresh(&mut o, "src/c.test.rs");
    assert_eq!(top_file(&mut o), 4);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    // A file before it: `src/b.rs` is file 5 of Changes now (file 5 was
    // `src/c.test.rs`, in Tests).
    refresh(&mut o, "src/a2.rs");
    assert_eq!(
        panels(&mut o),
        [
            p("changes", &[1, 2, 4, 5]),
            p("tests", &[3, 6, 7]),
            p("generated", &[0])
        ]
    );
    assert_eq!(top_file(&mut o), 5);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
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

// M9, the panel switch (T7.13, ADR-0030): a header click lays both bodies
// out at the body's height H and clips each from its header's bottom edge
// on one track (the clips sum to H), the headers between gliding and both
// chevrons turning; the closing body stays live and inert under an
// occluding layer. Stepped by ADR-0030's protocol; SLIDE is this file's own
// bisection, and every expected value is quantized here.

/// `cubic-bezier(x1, y1, x2, y2)` at `t` by bisection on x.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let at = |a: f64, b: f64, s: f64| {
        3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..200 {
        let mid = (low + high) / 2.0;
        if at(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    at(y1, y2, (low + high) / 2.0)
}

/// SLIDE.
fn slide(t: f64) -> f64 {
    bezier(0.25, 1.0, 0.5, 1.0, t)
}

fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

/// The body's height in these tests, and its reveal: 150 + 0.1 · 400 ms.
const H: f32 = 400.0;
const REVEAL_MS: f64 = 190.0;

type Drawn = Rc<RefCell<Vec<Recorded>>>;

/// `repo` opened under Full app motion, its samples recorded, and the
/// window sized so the open body is [`H`] tall (it grows with the window).
fn open_swapping<'a>(cx: &'a mut TestAppContext, repo: &FixtureRepo) -> (Opened<'a>, Drawn) {
    let drawn: Drawn = Rc::default();
    cx.update(|cx| motion::record(drawn.clone(), cx));
    let mut o = open(cx, repo);
    set_policy(&mut o, MotionPolicy::Full);
    let key = open_panel(&mut o).expect("an open panel");
    let body = clip(&mut o, &key);
    let window = o.shell.cx.update(|window, _| window.viewport_size());
    crate::shell::resize_window(
        &mut o.shell,
        window.width.as_f32(),
        window.height.as_f32() + H - body,
    );
    assert_eq!(clip(&mut o, &key), H, "the open body is H tall");
    (o, drawn)
}

fn set_policy(o: &mut Opened, policy: MotionPolicy) {
    o.shell
        .cx
        .update(|_, cx| motion::set_override(Some(policy), cx));
}

/// `v` on the test window's device pixel grid.
fn quantized(o: &mut Opened, v: f64) -> f32 {
    let scale = f64::from(o.shell.cx.update(|window, _| window.scale_factor()));
    ((v * scale).round() / scale) as f32
}

/// The opening clip at `at` ms of a 190 ms swap.
fn opening_at(o: &mut Opened, at: f64) -> f32 {
    quantized(o, f64::from(H) * slide(at / REVEAL_MS))
}

/// The visible height of panel `key`'s body (its clip); 0 when it is not
/// painted.
fn clip(o: &mut Opened, key: &str) -> f32 {
    painted(o.shell.cx, &format!("tree-panel-body-{key}")).map_or(0.0, |b| b.size.height.as_f32())
}

fn body_top(o: &mut Opened, key: &str) -> f32 {
    crate::shell::bounds(o.shell.cx, &format!("tree-panel-body-{key}"))
        .top()
        .as_f32()
}

fn header_top(o: &mut Opened, key: &str) -> f32 {
    crate::shell::bounds(o.shell.cx, &format!("tree-panel-{key}"))
        .top()
        .as_f32()
}

fn header_bottom(o: &mut Opened, key: &str) -> f32 {
    crate::shell::bounds(o.shell.cx, &format!("tree-panel-{key}"))
        .bottom()
        .as_f32()
}

/// The last recorded sample of `id`.
fn last(drawn: &Drawn, id: &str) -> Recorded {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    drawn
        .borrow()
        .iter()
        .rev()
        .find(|r| r.id == id)
        .cloned()
        .unwrap_or_else(|| panic!("nothing drawn for {id:?}"))
}

/// The angle panel `key`'s chevron was last drawn at, in degrees.
fn chevron(drawn: &Drawn, key: &str) -> f32 {
    last(drawn, &format!("tree-panel-chevron-{key}")).rotation
}

/// The opacity panel `key`'s list was last drawn at.
fn list_opacity(drawn: &Drawn, key: &str) -> f32 {
    last(drawn, &format!("tree-panel-body-{key}")).opacity
}

/// A pointer click on panel `key`'s header where it was last painted, and
/// the commit frame.
fn press_header(o: &mut Opened, key: &str) {
    let at = crate::shell::bounds(o.shell.cx, &format!("tree-panel-{key}")).center();
    o.shell.cx.simulate_click(at, Modifiers::none());
    frame(o.shell.cx);
}

/// Whether the window stops asking for frames without the clock moving: a
/// jump's own loads may take a few, a running motion asks on every frame.
fn goes_quiet(o: &mut Opened) -> bool {
    (0..20).any(|_| requested_frames(o.shell.cx) == 0)
}

fn swapping(o: &mut Opened) -> bool {
    o.tree
        .read_with(o.shell.cx, |t, _| t.panels().is_swapping())
}

fn panel_key(o: &mut Opened, key: &str) -> polygloss_app::tree::panels::PanelKey {
    o.tree.read_with(o.shell.cx, |t, _| {
        t.panels()
            .panels()
            .iter()
            .find(|p| p.key().to_string() == key)
            .expect("the panel")
            .key()
            .clone()
    })
}

/// Opens panel `key` at once, as a jump does.
fn open_at_once(o: &mut Opened, key: &str) {
    let key = panel_key(o, key);
    let tree = o.tree.clone();
    o.shell
        .cx
        .update(|_, cx| tree.update(cx, |t, cx| t.open_panel(&key, cx)));
    draw(o.shell.cx);
}

/// The tops of `headers` with panel `open` open, measured at rest; then
/// the panel that was open opens again.
fn tops_with_open(o: &mut Opened, open: &str, headers: &[&str]) -> Vec<f32> {
    let was = open_panel(o).expect("an open panel");
    open_at_once(o, open);
    let tops = headers.iter().map(|k| header_top(o, k)).collect();
    open_at_once(o, &was);
    tops
}

#[gpui_kit::test]
fn header_click_trades_heights(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, drawn) = open_swapping(cx, &repo);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    // Changes (open, on top) closes and Generated (the last) opens: the
    // headers of Tests and Generated, between them, glide up by H.
    let between = ["tests", "generated"];
    let start: Vec<f32> = between.iter().map(|k| header_top(&mut o, k)).collect();
    let end = tops_with_open(&mut o, "generated", &between);
    for (s, e) in start.iter().zip(&end) {
        assert_eq!(s - e, H, "H between a header's two places");
    }
    let changes_top = header_top(&mut o, "changes");
    // At rest, the open body ends this far above the next header (its
    // separator).
    let gap = header_top(&mut o, "tests") - (body_top(&mut o, "changes") + H);

    // The commit frame: the model is final (Generated open), the paint is
    // where it was.
    press_header(&mut o, "generated");
    assert_eq!(open_panel(&mut o).as_deref(), Some("generated"));
    assert!(swapping(&mut o));
    assert_eq!(clip(&mut o, "changes"), H, "the commit frame");
    assert_eq!(clip(&mut o, "generated"), 0.0);
    for (k, s) in between.iter().zip(&start) {
        assert_eq!(header_top(&mut o, k), *s, "{k} on the commit frame");
    }
    assert_eq!(chevron(&drawn, "changes"), 0.0);
    assert_eq!(chevron(&drawn, "generated"), -90.0);

    for (at, label) in [(16.667, "the first step"), (REVEAL_MS / 2.0, "½")] {
        if at < 20.0 {
            step_to(o.shell.cx, FIRST_STEP);
        } else {
            advance(o.shell.cx, ms(at) - FIRST_STEP);
        }
        let opening = opening_at(&mut o, at);
        assert!(0.0 < opening && opening < H, "{label}: {opening}");
        assert_eq!(clip(&mut o, "generated"), opening, "{label}");
        assert_eq!(
            clip(&mut o, "changes") + clip(&mut o, "generated"),
            H,
            "{label}: the clips sum to H"
        );
        for (k, e) in between.iter().zip(&end) {
            assert_eq!(header_top(&mut o, k), e + H - opening, "{label}: {k}");
        }
        // Each body hangs from its header's bottom edge; the closing one
        // ends where it did at rest before the next header.
        assert_eq!(header_top(&mut o, "changes"), changes_top, "{label}");
        assert_eq!(
            body_top(&mut o, "changes"),
            header_bottom(&mut o, "changes")
        );
        assert_eq!(
            body_top(&mut o, "generated"),
            header_bottom(&mut o, "generated")
        );
        assert_eq!(
            header_top(&mut o, "tests") - (body_top(&mut o, "changes") + clip(&mut o, "changes")),
            gap,
            "{label}"
        );
        // Both chevrons share the track.
        let share = slide(at / REVEAL_MS) as f32;
        let (opening_deg, closing_deg) = (chevron(&drawn, "generated"), chevron(&drawn, "changes"));
        assert!(
            (opening_deg + 90.0 * (1.0 - share)).abs() < 1e-3,
            "{label}: {opening_deg}"
        );
        assert!(
            (closing_deg + 90.0 * share).abs() < 1e-3,
            "{label}: {closing_deg}"
        );
    }

    // Settled after 190 ms: Changes' body is gone, every header at its
    // place, no frame asked for.
    advance(o.shell.cx, ms(REVEAL_MS / 2.0));
    assert!(!swapping(&mut o));
    assert!(painted(o.shell.cx, "tree-panel-body-changes").is_none());
    assert_eq!(clip(&mut o, "generated"), H);
    for (k, e) in between.iter().zip(&end) {
        assert_eq!(header_top(&mut o, k), *e, "{k} settled");
    }
    assert_eq!(chevron(&drawn, "generated"), 0.0);
    assert_eq!(chevron(&drawn, "changes"), -90.0);
    assert_eq!(requested_frames(o.shell.cx), 0, "nothing runs");
}

#[gpui_kit::test]
fn header_click_upward_glides_the_headers_down(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    open_at_once(&mut o, "generated");
    // Generated (the last) closes, Changes (on top) opens: the headers of
    // Tests and Generated glide down by H, and Generated's body rides its
    // header.
    let between = ["tests", "generated"];
    let start: Vec<f32> = between.iter().map(|k| header_top(&mut o, k)).collect();
    let end = tops_with_open(&mut o, "changes", &between);
    let gap = {
        open_at_once(&mut o, "changes");
        let gap = header_top(&mut o, "tests") - (body_top(&mut o, "changes") + H);
        open_at_once(&mut o, "generated");
        gap
    };
    press_header(&mut o, "changes");
    assert_eq!(clip(&mut o, "generated"), H, "the commit frame");
    assert_eq!(clip(&mut o, "changes"), 0.0);
    step_to(o.shell.cx, ms(REVEAL_MS / 2.0));
    let opening = opening_at(&mut o, REVEAL_MS / 2.0);
    assert_eq!(clip(&mut o, "changes"), opening);
    assert_eq!(clip(&mut o, "generated"), H - opening);
    for ((k, s), e) in between.iter().zip(&start).zip(&end) {
        let top = header_top(&mut o, k);
        assert_eq!(top, s + opening, "{k}");
        assert!(*s < top && top < *e, "{k} between its places");
    }
    assert_eq!(
        body_top(&mut o, "changes"),
        header_bottom(&mut o, "changes")
    );
    assert_eq!(
        body_top(&mut o, "generated"),
        header_bottom(&mut o, "generated")
    );
    assert_eq!(
        header_top(&mut o, "tests") - (body_top(&mut o, "changes") + opening),
        gap,
        "the opening clip ends where the open body does at rest"
    );
}

#[gpui_kit::test]
fn the_closing_body_is_inert(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    let changes = o.tree.read_with(o.shell.cx, |t, _| {
        t.panels().panels()[0].tree_state().clone()
    });
    let state = |o: &mut Opened| {
        (
            changes.read_with(o.shell.cx, |s, _| s.selected_index()),
            top_file(o),
            o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs()),
        )
    };
    let before = state(&mut o);
    press_header(&mut o, "tests");
    step_to(o.shell.cx, FIRST_STEP);
    // `src/b.rs`, in the shrinking Changes body, still shows.
    let row = crate::shell::bounds(o.shell.cx, "tree-row-f:4");
    let shown_to = body_top(&mut o, "changes") + clip(&mut o, "changes");
    assert!(row.bottom().as_f32() < shown_to, "the row shows");
    o.shell.cx.simulate_click(row.center(), Modifiers::none());
    draw(o.shell.cx);
    assert_eq!(state(&mut o), before, "the click selected nothing");
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert_eq!(selected_file(&mut o), None);
}

#[gpui_kit::test]
fn follow_and_jumps_switch_instantly(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    let instant = |o: &mut Opened, key: &str, others: &[&str], what: &str| {
        assert_eq!(open_panel(o).as_deref(), Some(key), "{what}");
        assert!(!swapping(o), "{what}: no swap");
        for other in others {
            assert_eq!(clip(o, other), 0.0, "{what}: {other} closed at once");
        }
        assert!(goes_quiet(o), "{what}: no frame asked for");
    };
    // A jump into Tests' section.
    go_to_file(&mut o.shell, &o.tab, 3);
    instant(&mut o, "tests", &["changes"], "a jump");
    assert_eq!(clip(&mut o, "tests"), H);
    // Scrolling up into `src/b.rs`: the follow opens Changes.
    scroll_by(&mut o, -200.0);
    assert_eq!(top_file(&mut o), 4);
    instant(&mut o, "changes", &["tests"], "the follow");
    assert_eq!(clip(&mut o, "changes"), H);
    // A keyboard-initiated open.
    let tests = panel_key(&mut o, "tests");
    let tree = o.tree.clone();
    o.shell.cx.update(|window, cx| {
        tree.update(cx, |t, cx| {
            t.open_panel_by(&tests, Initiator::Keyboard, window, cx)
        })
    });
    frame(o.shell.cx);
    instant(&mut o, "tests", &["changes"], "the keyboard");
    assert_eq!(clip(&mut o, "tests"), H);
    // The filter leaving only Changes' files opens Changes.
    set_query(&mut o, "b.rs");
    instant(&mut o, "changes", &["tests"], "the filter");
    set_query(&mut o, "");
    // A jump while a header's switch runs ends it: Tests opens whole at
    // once, and neither gliding body is left.
    press_header(&mut o, "generated");
    step_to(o.shell.cx, FIRST_STEP);
    assert!(swapping(&mut o));
    go_to_file(&mut o.shell, &o.tab, 3);
    instant(
        &mut o,
        "tests",
        &["changes", "generated"],
        "a jump mid-switch",
    );
    assert_eq!(clip(&mut o, "tests"), H);
}

#[gpui_kit::test]
fn a_third_click_settles_the_swap(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    let tests_rest = tops_with_open(&mut o, "generated", &["tests"])[0];
    press_header(&mut o, "generated");
    step_to(o.shell.cx, FIRST_STEP);
    // Tests' header is on its way up; a click where it is painted hits it.
    assert!(
        header_top(&mut o, "tests") > tests_rest + 1.0,
        "Tests' header is moving"
    );
    press_header(&mut o, "tests");
    // The first swap ended (Generated whole, Changes gone); the click's own
    // swap starts from there and still runs after the mouse up.
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(swapping(&mut o), "the click's swap runs on");
    assert!(painted(o.shell.cx, "tree-panel-body-changes").is_none());
    assert_eq!(clip(&mut o, "generated"), H, "the new swap's commit frame");
    assert_eq!(clip(&mut o, "tests"), 0.0);
    step_to(o.shell.cx, FIRST_STEP);
    let opening = opening_at(&mut o, 16.667);
    assert_eq!(clip(&mut o, "tests"), opening);
    assert_eq!(clip(&mut o, "generated"), H - opening);
}

#[gpui_kit::test]
fn the_opened_list_takes_the_keyboard_at_the_commit(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    let tree = o.tree.clone();
    let in_tree = |o: &mut Opened| {
        o.shell
            .cx
            .update(|window, cx| tree.read(cx).contains_focus(window, cx))
    };
    assert!(!in_tree(&mut o), "the diff has the keyboard");
    press_header(&mut o, "tests");
    assert!(swapping(&mut o), "the commit frame");
    assert!(in_tree(&mut o), "the opened list has the keyboard");
    // `n` walks Tests' files; its key down settles the swap first.
    o.shell.cx.simulate_keystrokes("n");
    frame(o.shell.cx);
    assert!(!swapping(&mut o), "a key down settles it");
    assert_eq!(selected_file(&mut o), Some(3));
    assert_eq!(clip(&mut o, "tests"), H);
}

#[gpui_kit::test]
fn each_bodys_scroll_offset_holds_through_the_swap(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // 0–29 `src/m00.rs`…, 30–59 `tests/t00.rs`…: each list taller than H.
    let paths: Vec<String> = (0..30)
        .map(|i| format!("src/m{i:02}.rs"))
        .chain((0..30).map(|i| format!("tests/t{i:02}.rs")))
        .collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    let repo = repo_with(&paths, 3);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    let (changes, tests) = o.tree.read_with(o.shell.cx, |t, _| {
        let p = t.panels().panels();
        (p[0].tree_state().clone(), p[1].tree_state().clone())
    });
    let offset = |o: &mut Opened, state: &Entity<gpui_kit::component::tree::TreeState>| {
        state.read_with(o.shell.cx, |s, _| {
            s.scroll_handle().0.borrow().base_handle.offset().y
        })
    };
    // Changes scrolled to the viewport's file (the tree keeps the current
    // file's row in view), Tests by hand (the tree is a cached view:
    // notified, so the list applies the scroll).
    go_to_file(&mut o.shell, &o.tab, 20);
    open_at_once(&mut o, "tests");
    tests.update(o.shell.cx, |s, _| s.scroll_to_item(25, ScrollStrategy::Top));
    o.tree.update(o.shell.cx, |_, cx| cx.notify());
    draw(o.shell.cx);
    open_at_once(&mut o, "changes");
    let (c0, t0) = (offset(&mut o, &changes), offset(&mut o, &tests));
    assert!(
        c0 < px(0.) && t0 < px(0.),
        "both lists scrolled: {c0:?} {t0:?}"
    );
    // Where a row of each list sits in its body.
    let in_body = |o: &mut Opened, row: &str, key: &str| {
        crate::shell::bounds(o.shell.cx, row).top().as_f32() - body_top(o, key)
    };
    let changes_row = in_body(&mut o, "tree-row-f:12", "changes");

    press_header(&mut o, "tests");
    let tests_row = in_body(&mut o, "tree-row-f:55", "tests");
    let check = |o: &mut Opened, label: &str| {
        assert_eq!(offset(o, &changes), c0, "Changes: {label}");
        assert_eq!(offset(o, &tests), t0, "Tests: {label}");
        assert_eq!(
            in_body(o, "tree-row-f:12", "changes"),
            changes_row,
            "{label}"
        );
        assert_eq!(in_body(o, "tree-row-f:55", "tests"), tests_row, "{label}");
    };
    check(&mut o, "the commit frame");
    step_to(o.shell.cx, FIRST_STEP);
    check(&mut o, "the first step");
    advance(o.shell.cx, ms(REVEAL_MS / 2.0) - FIRST_STEP);
    check(&mut o, "½");
    advance(o.shell.cx, ms(REVEAL_MS / 2.0));
    assert!(!swapping(&mut o));
    assert_eq!(offset(&mut o, &tests), t0, "settled");
    open_at_once(&mut o, "changes");
    assert_eq!(offset(&mut o, &changes), c0, "Changes kept its place");
}

#[gpui_kit::test]
fn header_click_reduced_fades_the_incoming_body(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, drawn) = open_swapping(cx, &repo);
    let between = ["tests", "generated"];
    let end = tops_with_open(&mut o, "generated", &between);
    set_policy(&mut o, MotionPolicy::Reduced);
    // An instant swap; Generated's list fades in over QUICK.
    press_header(&mut o, "generated");
    for (at, label) in [
        (0.0, "the commit frame"),
        (16.667, "+1 frame"),
        (75.0, "½ of QUICK"),
    ] {
        if at == 16.667 {
            step_to(o.shell.cx, FIRST_STEP);
        } else if at == 75.0 {
            advance(o.shell.cx, ms(75.0) - FIRST_STEP);
        }
        assert!(
            painted(o.shell.cx, "tree-panel-body-changes").is_none(),
            "{label}"
        );
        assert_eq!(clip(&mut o, "generated"), H, "{label}");
        for (k, e) in between.iter().zip(&end) {
            assert_eq!(header_top(&mut o, k), *e, "{label}: {k} at its place");
        }
        assert_eq!(chevron(&drawn, "generated"), 0.0, "{label}");
        assert_eq!(chevron(&drawn, "changes"), -90.0, "{label}");
        assert_eq!(
            last(&drawn, "tree-panel-body-generated").offset,
            point(px(0.), px(0.)),
            "{label}"
        );
    }
    let half = list_opacity(&drawn, "generated");
    assert!(0.0 < half && half < 1.0, "fading in: {half}");
    advance(o.shell.cx, ms(75.0));
    assert_eq!(
        list_opacity(&drawn, "generated"),
        1.0,
        "faded in after 150 ms"
    );
    assert!(!swapping(&mut o));
    assert_eq!(requested_frames(o.shell.cx), 0);

    // Off (the harnesses): settled on the commit frame.
    set_policy(&mut o, MotionPolicy::Off);
    press_header(&mut o, "tests");
    assert!(!swapping(&mut o));
    assert_eq!(clip(&mut o, "tests"), H);
    assert_eq!(list_opacity(&drawn, "tests"), 1.0);
    assert_eq!(requested_frames(o.shell.cx), 0);
}

#[gpui_kit::test]
fn the_swap_freezes_under_a_mouse_down_and_settles_on_input(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    // The footer's corner, left of its text: pressing it does nothing.
    let blank =
        crate::shell::bounds(o.shell.cx, "file-tree-pane").bottom_left() + point(px(4.), px(-4.));
    let back = |o: &mut Opened| {
        set_policy(o, MotionPolicy::Off);
        open_at_once(o, "changes");
        set_policy(o, MotionPolicy::Full);
    };

    // A pointer move leaves it running.
    press_header(&mut o, "tests");
    step_to(o.shell.cx, FIRST_STEP);
    o.shell
        .cx
        .simulate_mouse_move(blank, None, Modifiers::none());
    advance(o.shell.cx, ms(REVEAL_MS / 2.0) - FIRST_STEP);
    let half = opening_at(&mut o, REVEAL_MS / 2.0);
    assert_eq!(clip(&mut o, "tests"), half, "running after a pointer move");

    // A mouse down freezes it, asking for no frame; its mouse up settles
    // it.
    o.shell
        .cx
        .simulate_mouse_down(blank, MouseButton::Left, Modifiers::none());
    for _ in 0..3 {
        advance(o.shell.cx, FIRST_STEP);
        assert_eq!(clip(&mut o, "tests"), half, "frozen");
    }
    assert_eq!(requested_frames(o.shell.cx), 0, "frozen: no frames");
    o.shell
        .cx
        .simulate_mouse_up(blank, MouseButton::Left, Modifiers::none());
    frame(o.shell.cx);
    assert!(!swapping(&mut o), "settled at the mouse up");
    assert_eq!(clip(&mut o, "tests"), H);

    // A scroll settles it.
    back(&mut o);
    press_header(&mut o, "tests");
    step_to(o.shell.cx, FIRST_STEP);
    let pane = crate::shell::bounds(o.shell.cx, "file-tree-pane").center();
    o.shell.cx.simulate_event(ScrollWheelEvent {
        position: pane,
        delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
        ..Default::default()
    });
    frame(o.shell.cx);
    assert!(!swapping(&mut o), "a scroll settles it");
    assert_eq!(clip(&mut o, "tests"), H);
    assert_eq!(requested_frames(o.shell.cx), 0);

    // A resize settles it.
    back(&mut o);
    press_header(&mut o, "tests");
    step_to(o.shell.cx, FIRST_STEP);
    let window = o.shell.cx.update(|window, _| window.viewport_size());
    o.shell
        .cx
        .simulate_resize(gpui_kit::size(window.width + px(40.), window.height));
    frame(o.shell.cx);
    assert!(!swapping(&mut o), "a resize settles it");
    assert_eq!(requested_frames(o.shell.cx), 0);
}

/// The accordion's perf driver (`perf::motion`) switches between the first
/// two panels by pointer: a swap runs, and the window's settling sees it.
#[gpui_kit::test]
fn accordion_driver_switches_the_first_two_panels_by_pointer(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(60);
    let (mut o, _drawn) = open_swapping(cx, &repo);
    let driver = polygloss_app::perf::motion::DRIVERS
        .iter()
        .find(|d| d.name == "accordion")
        .expect("the accordion driver");
    o.shell.cx.update(|window, cx| (driver.open)(window, cx));
    frame(o.shell.cx);
    assert_eq!(open_panel(&mut o).as_deref(), Some("tests"));
    assert!(swapping(&mut o), "by pointer");
    o.shell.cx.simulate_keystrokes("escape");
    assert!(!swapping(&mut o), "settled by a key down");
    o.shell.cx.update(|window, cx| (driver.close)(window, cx));
    frame(o.shell.cx);
    assert_eq!(open_panel(&mut o).as_deref(), Some("changes"));
    assert!(swapping(&mut o));
}
