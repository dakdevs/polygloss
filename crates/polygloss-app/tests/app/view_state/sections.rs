//! View state and category sections (T6.14, design §11.12, §11.15): the
//! open sections saved and restored per diff, saved state winning over the
//! default and waiting-question rules, and a restored line in a closed
//! section landing on its band; the tree expansion of every sidebar panel
//! (T6.15).

use gpui_kit::TestAppContext;
use polygloss_core::review::{OpenRequest, ScrollAnchorState, ViewState};
use polygloss_diff::Side;
use polygloss_viewport::ScrollAnchor;

use super::{close, compare, restart, row_below_header, settle, stored};
use crate::categories::{
    anchor as viewport_anchor, click_band, mixed_repo, repo_with, section_id, sections,
    top_line as shown_line,
};
use crate::shell::{Shell, start};
use crate::support::Sandbox;

/// Opens `req` in core (storing its diff) and saves `state` as its view
/// state, as an earlier session would have.
fn saved_state(shell: &mut Shell, req: &OpenRequest, state: ViewState) {
    let opened = shell.core.open(req).unwrap();
    shell.core.save_view_state(&opened.diff_id, &state).unwrap();
}

fn section(category: &str, files: &[u32], open: bool) -> (String, Vec<u32>, bool) {
    (category.to_owned(), files.to_vec(), open)
}

/// The sidebar's open panel and its selected file (design §11.5: the open
/// panel follows the viewport, whose top file's row is selected).
fn tree_follows(
    shell: &mut Shell,
    tab: &gpui_kit::Entity<polygloss_app::review_tab::ReviewTab>,
) -> (Option<String>, Option<u32>) {
    tab.read_with(shell.cx, |t, cx| {
        let tree = polygloss_app::tree::file_tree(t).unwrap().read(cx);
        (
            tree.panels().open_key().map(ToString::to_string),
            tree.selected_file(),
        )
    })
}

#[gpui_kit::test]
fn open_sections_round_trip_through_view_state(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    // Untouched: nothing about sections is saved.
    settle(&mut shell);
    assert_eq!(stored(&mut shell, &tab).and_then(|s| s.open_sections), None);

    let tests = section_id(&mut shell, &tab, "tests");
    click_band(
        &mut shell,
        &tab,
        polygloss_viewport::ControlAction::SectionToggle(tests),
    );
    settle(&mut shell);
    assert_eq!(
        stored(&mut shell, &tab).and_then(|s| s.open_sections),
        Some(vec!["tests".to_owned()])
    );
    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(
        sections(&mut shell, &tab),
        [
            section("tests", &[3, 5], true),
            section("generated", &[0], false)
        ]
    );
}

#[gpui_kit::test]
fn saved_open_sections_win_over_the_default_rule(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let req = compare(&repo);
    saved_state(
        &mut shell,
        &req,
        ViewState {
            open_sections: Some(vec!["generated".into()]),
            ..ViewState::default()
        },
    );
    // A question waiting on the human in `tests/it.rs` would open Tests; the
    // saved state wins.
    let opened = shell.core.open(&req).unwrap();
    let blobs = polygloss_core::objects::BlobReader::open(&opened.repo).unwrap();
    shell
        .core
        .create_thread(
            &polygloss_core::review::NewThread {
                review_id: opened.review_id.clone(),
                diff_id: opened.diff_id.clone(),
                subject: crate::threads::line("tests/it.rs", Side::New, 1, 1),
                kind: polygloss_core::review::ThreadKind::Question,
                body_md: "Needed?".into(),
                author: crate::threads::agent(),
            },
            &blobs,
        )
        .unwrap();
    let tab = shell.open(req).unwrap();
    assert_eq!(
        sections(&mut shell, &tab),
        [
            section("tests", &[3, 5], false),
            section("generated", &[0], true)
        ]
    );

    // Every file categorized would open them all; a saved `[]` keeps them
    // closed.
    let all = repo_with(&["Cargo.lock", "tests/it.rs"], 3);
    let req = compare(&all);
    saved_state(
        &mut shell,
        &req,
        ViewState {
            open_sections: Some(Vec::new()),
            ..ViewState::default()
        },
    );
    let tab = shell.open(req).unwrap();
    assert_eq!(
        sections(&mut shell, &tab),
        [
            section("tests", &[1], false),
            section("generated", &[0], false)
        ]
    );
}

#[gpui_kit::test]
fn restored_anchor_in_a_closed_section_lands_on_its_band(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut shell = start(cx);
    let req = compare(&repo);
    saved_state(
        &mut shell,
        &req,
        ViewState {
            scroll_anchor: Some(ScrollAnchorState {
                path: "src/a.test.rs".into(),
                side: Side::New,
                line: 40,
            }),
            ..ViewState::default()
        },
    );
    let tab = shell.open(req).unwrap();
    // The restore never opens a section: the view is on Tests' band.
    assert_eq!(
        sections(&mut shell, &tab)[0],
        section("tests", &[3, 5], false)
    );
    assert_eq!(
        viewport_anchor(&mut shell, &tab),
        ScrollAnchor {
            file_idx: 3,
            row: polygloss_viewport::RowKey::Lead,
            offset_px: 0.0,
        }
    );
    // The band names `src/a.test.rs`: the sidebar opens Tests on its row.
    assert_eq!(
        tree_follows(&mut shell, &tab),
        (Some("tests".to_owned()), Some(3))
    );
}

#[gpui_kit::test]
fn restored_anchor_in_a_saved_open_section_keeps_its_line(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut shell = start(cx);
    let req = compare(&repo);
    saved_state(
        &mut shell,
        &req,
        ViewState {
            scroll_anchor: Some(ScrollAnchorState {
                path: "src/a.test.rs".into(),
                side: Side::New,
                line: 40,
            }),
            open_sections: Some(vec!["tests".into()]),
            ..ViewState::default()
        },
    );
    let tab = shell.open(req).unwrap();
    assert_eq!(
        sections(&mut shell, &tab)[0],
        section("tests", &[3, 5], true)
    );
    assert_eq!(shown_line(&mut shell, &tab), Some((3, Side::New, 39)));
    assert!(row_below_header(&mut shell, &tab).contains("src/a.test.rs line 40"));
    assert_eq!(
        tree_follows(&mut shell, &tab),
        (Some("tests".to_owned()), Some(3))
    );
}

#[gpui_kit::test]
fn fresh_open_with_a_changed_generated_verdict_keeps_the_restored_view_state(
    cx: &mut TestAppContext,
) {
    let _sb = Sandbox::isolate();
    // `uv.lock` is in geld's lockfiles, not in v1's list: stored bit 0, yet
    // generated now.
    let repo = repo_with(&["src/a.rs", "src/b.rs", "uv.lock"], 120);
    let mut shell = start(cx);
    let req = compare(&repo);
    saved_state(
        &mut shell,
        &req,
        ViewState {
            scroll_anchor: Some(ScrollAnchorState {
                path: "src/b.rs".into(),
                side: Side::New,
                line: 50,
            }),
            collapsed: vec!["src/a.rs".into()],
            ..ViewState::default()
        },
    );
    let tab = shell.open(req).unwrap();
    let (stored_bit, shown_bit, collapsed) = tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        (
            t.opened.files[2].generated,
            v.document().files()[2].generated,
            v.collapsed(),
        )
    });
    assert!(!stored_bit, "v1's list has no uv.lock");
    assert!(shown_bit, "uv.lock is generated");
    assert_eq!(collapsed, [0]);
    assert_eq!(shown_line(&mut shell, &tab), Some((1, Side::New, 49)));
    assert!(row_below_header(&mut shell, &tab).contains("src/b.rs line 50"));
    assert_eq!(
        sections(&mut shell, &tab),
        [section("generated", &[2], false)]
    );
}

/// The expansion names of `tab`'s tree ([`FileTree::expanded_dirs`]).
fn expanded(
    shell: &mut Shell,
    tab: &gpui_kit::Entity<polygloss_app::review_tab::ReviewTab>,
) -> Vec<String> {
    tab.read_with(shell.cx, |t, cx| {
        polygloss_app::tree::file_tree(t)
            .unwrap()
            .read(cx)
            .expanded_dirs()
    })
}

/// T6.15: `tree_expanded` covers every panel of the Files accordion:
/// Changes' folders plainly, a category panel's after its `/<category>`
/// entry as `/<category>/<path>`. A review reopened restores them before
/// its first frame.
#[gpui_kit::test]
fn tree_expansion_of_every_panel_round_trips_through_view_state(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // Changes: `docs/x.md`, `src/a.rs`, `src/b.rs`; Tests: `src/a.test.rs`,
    // `tests/it.rs`; Generated: `Cargo.lock`.
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(
        expanded(&mut shell, &tab),
        [
            "docs",
            "src",
            "/tests",
            "/tests/src",
            "/tests/tests",
            "/generated"
        ]
    );
    // Collapse the Tests panel's `src/` (not Changes' `src/`).
    crate::shell::click(shell.cx, "tree-panel-tests");
    crate::shell::click(shell.cx, "tree-row-d:src");
    let saved = ["docs", "src", "/tests", "/tests/tests", "/generated"];
    assert_eq!(expanded(&mut shell, &tab), saved);
    settle(&mut shell);
    assert_eq!(
        stored(&mut shell, &tab).and_then(|s| s.tree_expanded),
        Some(saved.map(String::from).to_vec())
    );

    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(expanded(&mut shell, &tab), saved);
}

/// A state saved before panels existed names Changes' folders only: they
/// restore as saved, and a category panel it does not name (no
/// `/<category>` entry) keeps every folder expanded.
#[gpui_kit::test]
fn a_tree_expansion_without_a_panels_entry_leaves_that_panel_expanded(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let req = compare(&repo);
    saved_state(
        &mut shell,
        &req,
        ViewState {
            // `docs/` collapsed.
            tree_expanded: Some(vec!["src".into()]),
            ..ViewState::default()
        },
    );
    let tab = shell.open(req).unwrap();
    assert_eq!(
        expanded(&mut shell, &tab),
        ["src", "/tests", "/tests/src", "/tests/tests", "/generated"]
    );
}

/// A Changes folder may be named like a category panel's folder in the
/// old `<category>:<path>` form (`tests:src`); the two restore apart.
#[gpui_kit::test]
fn a_changes_folder_never_shares_a_name_with_a_panels_folder(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // Changes: `tests:src/x.rs`; Tests: `src/a.test.rs`.
    let repo = repo_with(&["src/a.test.rs", "tests:src/x.rs"], 3);
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    // Collapse Changes' `tests:src/`; Tests' `src/` stays expanded.
    crate::shell::click(shell.cx, "tree-row-d:tests:src");
    let saved = ["/tests", "/tests/src"];
    assert_eq!(expanded(&mut shell, &tab), saved);
    settle(&mut shell);
    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(expanded(&mut shell, &tab), saved);
}
