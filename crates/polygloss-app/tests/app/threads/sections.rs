//! Threads and category sections (T6.15, design §11.6 "Sections"): the
//! panel and `.` / `,` order threads by the files' display rank, so a
//! thread in a section comes after every main file's, and going to one in a
//! closed section opens it and its sidebar panel.

use gpui_kit::Entity;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads::panel;
use polygloss_core::review::ThreadKind;
use polygloss_diff::Side;
use polygloss_viewport::CursorPos;

use super::{create, cursor, human, keys, line, model, reload};
use crate::shell::{Shell, compare_req, start};
use crate::support::{FixtureRepo, Sandbox};

/// The mixed fixture (`categories::MIXED`) with an open thread on line 3
/// of each of `paths` (new side); returns the thread ids, in `paths` order.
fn mixed_with_threads(
    shell: &mut Shell,
    paths: &[&str],
) -> (FixtureRepo, Entity<ReviewTab>, Vec<String>) {
    let repo = crate::categories::mixed_repo(20);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let ids = paths
        .iter()
        .map(|p| {
            create(
                shell,
                &tab,
                line(p, Side::New, 3, 3),
                ThreadKind::Comment,
                p,
                human(),
            )
        })
        .collect();
    reload(shell, &tab);
    (repo, tab, ids)
}

/// The panel lists threads by the files' display rank, so a thread in the
/// Tests section comes after every main file's.
#[gpui_kit::test]
fn threads_panel_orders_by_display_rank(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    // Git order: `src/a.rs` (2), `src/a.test.rs` (3), `src/b.rs` (4);
    // display order puts the Tests file last.
    let (_repo, tab, ids) =
        mixed_with_threads(&mut shell, &["src/a.rs", "src/a.test.rs", "src/b.rs"]);
    let m = model(&mut shell, &tab);
    let rows = m.read_with(shell.cx, panel::rows);
    let listed: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(listed, [ids[0].as_str(), ids[2].as_str(), ids[1].as_str()]);
}

/// `.` / `,` walk the display order and reach a thread in a closed
/// section, opening it (and its sidebar panel).
#[gpui_kit::test]
fn dot_and_comma_reach_threads_in_closed_sections(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let (_repo, tab, _ids) = mixed_with_threads(&mut shell, &["src/a.test.rs", "src/b.rs"]);
    let tests = crate::categories::section_id(&mut shell, &tab, "tests");
    let open = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).section_open(tests))
    };
    assert_eq!(open(&mut shell), Some(false));
    crate::categories::set_cursor(&mut shell, &tab, 2, 0);
    crate::categories::focus_viewport(&mut shell, &tab);
    let at = |file_idx| CursorPos {
        file_idx,
        side: Side::New,
        line: 2,
        range_start: None,
    };
    // `src/b.rs` (display rank 2) before `src/a.test.rs` (rank 3).
    keys(&mut shell, ".");
    assert_eq!(cursor(&mut shell, &tab), Some(at(4)));
    assert_eq!(open(&mut shell), Some(false));
    keys(&mut shell, ".");
    assert_eq!(cursor(&mut shell, &tab), Some(at(3)));
    assert_eq!(open(&mut shell), Some(true));
    let panel = tab.read_with(shell.cx, |t, cx| {
        polygloss_app::tree::file_tree(t)
            .and_then(|tree| tree.read(cx).panels().open_key().map(ToString::to_string))
    });
    assert_eq!(panel.as_deref(), Some("tests"));
    keys(&mut shell, ",");
    assert_eq!(cursor(&mut shell, &tab), Some(at(4)));
    // Wrapping back from the first: the last in display order.
    keys(&mut shell, ",");
    assert_eq!(cursor(&mut shell, &tab), Some(at(3)));
}
