//! Swapping the diff a viewport shows (`DiffViewport::set_provider`, T3.11
//! live refresh): the new files replace the old ones in the same view,
//! unchanged files keep their loaded data, results of the old diff's work
//! never reach the new one, and the host restores the scroll position with
//! `scroll_to_anchor`.

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use polygloss_diff::Side;
use polygloss_viewport::{
    DiffViewport, FileFlags, FileState, LayoutMode, RowKey, ScrollAnchor, ScrollTarget,
};

use crate::support::*;

fn text(prefix: &str, n: u32) -> String {
    numbered(prefix, n).concat()
}

/// Three modified files of 200 lines, `b.txt` changing `b 100`.
fn before() -> Vec<Spec> {
    vec![
        Spec::modified(
            "a.txt",
            &text("a", 200),
            &text("a", 200).replace("a 5\n", "A 5\n"),
        ),
        Spec::modified(
            "b.txt",
            &text("b", 200),
            &text("b", 200).replace("b 100\n", "B 100\n"),
        ),
        Spec::modified(
            "c.txt",
            &text("c", 200),
            &text("c", 200).replace("c 9\n", "C 9\n"),
        ),
    ]
}

/// `before` after an edit: `a.txt` unchanged (same blobs), `b.txt` with
/// five more lines at its top, `c.txt` gone, `d.txt` added.
fn after() -> Vec<Spec> {
    let b_old = text("b", 200);
    let b_new = format!("{}{}", text("new", 5), b_old.replace("b 100\n", "B 100\n"));
    vec![
        Spec::modified(
            "a.txt",
            &text("a", 200),
            &text("a", 200).replace("a 5\n", "A 5\n"),
        ),
        Spec::modified("b.txt", &b_old, &b_new),
        Spec::added("d.txt", &text("d", 60)),
    ]
}

fn state(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, f: u32) -> FileState {
    view.read_with(cx, |v, _| v.document().state(f).clone())
}

#[gpui_kit::test]
fn set_provider_shows_the_new_files_in_the_same_view(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        MemProvider::new(before()),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    view.update(cx, |v, cx| {
        v.set_collapsed(0, true, cx);
        v.set_file_flags(
            vec![FileFlags {
                viewed: true,
                ..FileFlags::default()
            }],
            cx,
        );
        v.scroll_to(ScrollTarget::File(2), cx);
    });
    settle(cx);

    let next = MemProvider::new(after());
    view.update(cx, |v, cx| {
        v.set_provider(next.clone(), &[None, None, None], cx)
    });
    settle(cx);
    let (paths, collapsed, flags, anchor, layout) = view.read_with(cx, |v, _| {
        let paths: Vec<String> = v
            .document()
            .files()
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect();
        (
            paths,
            v.collapsed(),
            v.file_flags().to_vec(),
            v.anchor(),
            v.options().layout,
        )
    });
    assert_eq!(paths, ["a.txt", "b.txt", "d.txt"]);
    // Per-file state starts over; the options stay.
    assert!(collapsed.is_empty());
    assert_eq!(flags, vec![FileFlags::default(); 3]);
    assert_eq!(anchor, ScrollAnchor::default());
    assert_eq!(layout, LayoutMode::Unified);
    let rows = debug(&view, cx).visible_rows;
    assert_eq!(rows[0], "== a.txt");
    assert!(
        rows.iter().all(|r| !r.contains("c 9")),
        "nothing of the old diff is left: {rows:?}"
    );
}

#[gpui_kit::test]
fn set_provider_keeps_loaded_data_of_unchanged_files(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        MemProvider::new(before()),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    assert!(state(&view, cx, 0).is_materialized());

    let next = MemProvider::new(after());
    // a.txt is unchanged (same ids). A carry that names another file is
    // ignored (`MemProvider` numbers blobs by position, so b.txt's ids are
    // the same in both lists: only the paths tell c.txt apart).
    view.update(cx, |v, cx| {
        v.set_provider(next.clone(), &[Some(0), Some(2), None], cx)
    });
    assert!(
        state(&view, cx, 0).is_materialized(),
        "an unchanged file shows at once"
    );
    settle(cx);
    assert_eq!(
        next.loads_of(0),
        0,
        "a.txt is not read again: {:?}",
        next.load_order()
    );
    assert!(next.loads_of(1) > 0, "b.txt is loaded from the new diff");
    assert!(state(&view, cx, 1).is_materialized());
}

#[gpui_kit::test]
fn results_of_the_old_diff_never_reach_the_new_one(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // The first frame starts loads that have not run yet.
    let (view, cx) = open_idle(
        cx,
        MemProvider::new(before()),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    let next = MemProvider::new(after());
    view.update(cx, |v, cx| {
        v.set_provider(next.clone(), &[None, None, None], cx)
    });
    settle(cx);
    let rows = debug(&view, cx).visible_rows;
    assert!(rows.iter().any(|r| r.contains("new 0")), "{rows:?}");
    assert!(rows.iter().all(|r| !r.contains("c 9")), "{rows:?}");
    for f in 0..3 {
        assert!(state(&view, cx, f).is_materialized(), "file {f}");
    }
}

#[gpui_kit::test]
fn scroll_to_anchor_restores_a_line_after_the_swap(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        MemProvider::new(before()),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 1,
                side: Side::New,
                line: 100,
            },
            cx,
        )
    });
    settle(cx);
    let before_anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(
        before_anchor.row,
        RowKey::Line {
            side: Side::New,
            line: 100
        }
    );
    let before_rows = debug(&view, cx).visible_rows;

    // Five lines were inserted above: line 100 is line 105 now.
    let next = MemProvider::new(after());
    view.update(cx, |v, cx| {
        v.set_provider(next.clone(), &[Some(0), None, None], cx);
        v.scroll_to_anchor(
            ScrollAnchor {
                row: RowKey::Line {
                    side: Side::New,
                    line: 105,
                },
                ..before_anchor
            },
            cx,
        );
    });
    settle(cx);
    let anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(anchor.file_idx, 1);
    assert_eq!(
        anchor.row,
        RowKey::Line {
            side: Side::New,
            line: 105
        }
    );
    // The same text is at the top; only the new-side numbers moved by 5.
    let rows = debug(&view, cx).visible_rows;
    let texts = |rows: &[String]| -> Vec<String> {
        rows.iter()
            .map(|r| r.get(14..).unwrap_or(r).to_owned())
            .collect()
    };
    assert_eq!(texts(&rows)[..8], texts(&before_rows)[..8], "{rows:?}");
}
