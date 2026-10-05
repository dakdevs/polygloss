//! Tree rows (T6.11, design §11.5): the outline icons, right-aligned
//! `+a −d` and status letters, the Viewed slot at the row's end (OQ-43) and
//! the filter field that replaced the "FILES n" header.

use std::time::Duration;

use gpui_kit::{TestAppContext, px};
use polygloss_app::tree::model::ItemId;
use polygloss_app::tree::row::Check;
use polygloss_viewport::FileFlags;

use super::{
    Opened, PATHS, click, open_tree, row_labels, selected_row, settle_counts, top_file, tree_repo,
};
use crate::shell::{bounds, draw, hover, painted, unhover};
use crate::support::Sandbox;

/// The Viewed slot's glyph painted on row `key` (`"f:3"`, `"d:src"`), if
/// any: `circle`, `circle-check` or `circle-minus`.
fn slot_glyph(o: &mut Opened, key: &str) -> Option<&'static str> {
    let shown: Vec<&'static str> = ["circle", "circle-check", "circle-minus"]
        .into_iter()
        .filter(|g| painted(o.shell.cx, &format!("tree-slot-{key}: {g}")).is_some())
        .collect();
    assert!(shown.len() <= 1, "{key} paints {shown:?}");
    shown.first().copied()
}

/// The icon painted before row `key`'s name, if any.
fn row_icon(o: &mut Opened, key: &str) -> Option<&'static str> {
    let shown: Vec<&'static str> = [
        "folder",
        "folder-open",
        "folder-closed",
        "file",
        "file-text",
    ]
    .into_iter()
    .filter(|i| painted(o.shell.cx, &format!("tree-icon-{key}: {i}")).is_some())
    .collect();
    assert!(shown.len() <= 1, "{key} paints {shown:?}");
    shown.first().copied()
}

/// Pushes the Viewed flags: files `viewed` viewed, the rest not.
fn set_viewed(o: &mut Opened, viewed: &[usize]) {
    let mut flags = vec![FileFlags::default(); PATHS.len()];
    for &f in viewed {
        flags[f].viewed = true;
    }
    o.tree
        .update(o.shell.cx, |t, cx| t.set_file_flags(flags, cx));
    draw(o.shell.cx);
}

#[gpui_kit::test]
fn tree_rows_show_icons_then_right_aligned_stats_and_status(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    settle_counts(&mut o.shell, &o.tab, PATHS.len());
    // Hand-counted from `tree_repo`: each file's `+a −d` (both, zeros too,
    // as the reference shows `+430 −0`) and its status letter.
    let expected = [
        (0, "+1 −0", "M"),
        (1, "+60 −0", "A"),
        (2, "+60 −0", "A"),
        (3, "+1 −0", "M"),
        (4, "+0 −60", "D"),
        (5, "+1 −0", "M"),
        (6, "+0 −0", "R"),
    ];
    let mut ends = Vec::new();
    for (f, stats, letter) in expected {
        let key = format!("f:{f}");
        let row = bounds(o.shell.cx, &format!("tree-row-{key}"));
        assert_eq!(row.size.height, px(28.), "{key}");
        let icon = bounds(o.shell.cx, &format!("tree-icon-{key}: file"));
        let name = bounds(o.shell.cx, &format!("tree-name-{key}"));
        let stats = bounds(o.shell.cx, &format!("tree-stats-{f}: {stats}"));
        let status = bounds(o.shell.cx, &format!("tree-status-{f}: {letter}"));
        let slot = bounds(o.shell.cx, &format!("tree-check-{key}"));
        // Left to right: the icon, the name, then at the right `+a −d`, the
        // status letter and, last, the Viewed slot.
        assert!(icon.right() <= name.left(), "{key}");
        assert!(name.right() <= stats.left(), "{key}");
        assert!(stats.right() <= status.left(), "{key}");
        assert!(status.right() <= slot.left(), "{key}");
        assert!(slot.right() <= row.right(), "{key}");
        assert!(
            row.right() - slot.right() <= px(8.),
            "{key} ends at the row's end"
        );
        ends.push((stats.right(), status.right(), slot.left()));
    }
    // Right-aligned: the counts, letters and slots of every depth line up.
    assert!(ends.windows(2).all(|w| w[0] == w[1]), "{ends:?}");
    // A folder's slot lines up with its files'.
    assert_eq!(bounds(o.shell.cx, "tree-check-d:src").left(), ends[0].2);
    assert_eq!(bounds(o.shell.cx, "tree-row-d:src").size.height, px(28.));
}

#[gpui_kit::test]
fn unviewed_rows_at_rest_show_the_folder_and_file_icons(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    unhover(o.shell.cx);
    let dirs = ["d:docs/guide", "d:src", "d:src/app/ui"];
    for key in dirs {
        assert_eq!(row_icon(&mut o, key), Some("folder"), "{key}");
        assert_eq!(slot_glyph(&mut o, key), None, "{key}");
    }
    for f in 0..PATHS.len() {
        let key = format!("f:{f}");
        assert_eq!(row_icon(&mut o, &key), Some("file"), "{key}");
        assert_eq!(slot_glyph(&mut o, &key), None, "{key}");
    }
    // Collapsed folders show the same `folder` (the reference has no open
    // folder icon).
    o.tree.update(o.shell.cx, |t, cx| {
        t.set_expanded_dirs(["src".to_string()], cx)
    });
    draw(o.shell.cx);
    assert_eq!(
        row_labels(&mut o),
        [
            "README.md",
            "config.toml",
            "docs/guide",
            "src",
            "app/ui",
            "lib.rs",
            "new_name.rs"
        ]
    );
    for key in dirs {
        assert_eq!(row_icon(&mut o, key), Some("folder"), "{key}");
    }
    // A viewed file keeps its `file` icon: the slot at the row's end shows
    // the mark, and the icon never changes.
    set_viewed(&mut o, &[0]);
    unhover(o.shell.cx);
    assert_eq!(row_icon(&mut o, "f:0"), Some("file"));
    assert_eq!(slot_glyph(&mut o, "f:0"), Some("circle-check"));
}

#[gpui_kit::test]
fn row_hover_shows_the_viewed_circle(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    set_viewed(&mut o, &[5]);
    unhover(o.shell.cx);
    // At rest: nothing on unviewed rows; the partly viewed folder and the
    // viewed file show their marks.
    assert_eq!(slot_glyph(&mut o, "f:6"), None);
    assert_eq!(slot_glyph(&mut o, "d:src"), Some("circle-minus"));
    assert_eq!(slot_glyph(&mut o, "f:5"), Some("circle-check"));

    // Hovering a row anywhere (here its name) shows its empty circle, on
    // that row only.
    hover(o.shell.cx, "tree-name-f:6");
    assert_eq!(slot_glyph(&mut o, "f:6"), Some("circle"));
    assert_eq!(slot_glyph(&mut o, "f:3"), None);
    assert_eq!(slot_glyph(&mut o, "d:src"), Some("circle-minus"));
    assert_eq!(slot_glyph(&mut o, "f:5"), Some("circle-check"));
    hover(o.shell.cx, "tree-name-d:src/app/ui");
    assert_eq!(slot_glyph(&mut o, "d:src/app/ui"), Some("circle"));
    assert_eq!(slot_glyph(&mut o, "f:6"), None);
    unhover(o.shell.cx);
    assert_eq!(slot_glyph(&mut o, "d:src/app/ui"), None);

    // The slot's own tooltip says what a click does.
    let tooltip = |o: &mut Opened, key: &str| -> Option<&'static str> {
        hover(o.shell.cx, &format!("tree-check-{key}"));
        o.shell.cx.executor().advance_clock(Duration::from_secs(2));
        draw(o.shell.cx);
        let shown = ["Mark viewed (v)", "Mark unviewed (v)"]
            .into_iter()
            .find(|t| painted(o.shell.cx, &format!("tooltip: {t}")).is_some());
        unhover(o.shell.cx);
        shown
    };
    assert_eq!(tooltip(&mut o, "f:6"), Some("Mark viewed (v)"));
    assert_eq!(tooltip(&mut o, "f:5"), Some("Mark unviewed (v)"));
    assert_eq!(tooltip(&mut o, "d:src"), Some("Mark viewed (v)"));
}

#[gpui_kit::test]
fn slot_toggles_viewed_and_the_icon_never_does(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    let viewed = |o: &mut Opened| -> Vec<usize> {
        o.tree.read_with(o.shell.cx, |t, _| {
            t.file_flags()
                .iter()
                .enumerate()
                .filter(|(_, f)| f.viewed)
                .map(|(i, _)| i)
                .collect()
        })
    };
    let expanded = |o: &mut Opened| o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs());

    // One click on a file's slot marks it viewed; another unmarks it.
    click(&mut o, "tree-check-f:6");
    assert_eq!(viewed(&mut o), [6]);
    unhover(o.shell.cx);
    assert_eq!(slot_glyph(&mut o, "f:6"), Some("circle-check"));
    click(&mut o, "tree-check-f:6");
    assert!(viewed(&mut o).is_empty());

    // The file icon is never a toggle: it selects the row, as its name does.
    click(&mut o, "tree-icon-f:3: file");
    assert_eq!(selected_row(&mut o), Some(ItemId::File(3)));
    assert_eq!(top_file(&mut o), 3);
    assert!(viewed(&mut o).is_empty());

    // A folder's icon folds the folder like the rest of its row and marks
    // nothing.
    click(&mut o, "tree-icon-d:src/app/ui: folder");
    assert_eq!(expanded(&mut o), ["docs/guide", "src"]);
    assert!(viewed(&mut o).is_empty());

    // The folder's slot marks every file below it and never folds it.
    click(&mut o, "tree-check-d:src");
    assert_eq!(viewed(&mut o), [3, 4, 5, 6]);
    assert_eq!(expanded(&mut o), ["docs/guide", "src"]);
}

#[gpui_kit::test]
fn folder_slot_is_tristate(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    // Files viewed below `src` (3–6), its state, its slot at rest and with
    // its row hovered: hand-listed.
    type Case = (
        &'static [usize],
        Check,
        Option<&'static str>,
        Option<&'static str>,
    );
    let cases: [Case; 3] = [
        (&[], Check::Off, None, Some("circle")),
        (
            &[5],
            Check::Mixed,
            Some("circle-minus"),
            Some("circle-minus"),
        ),
        (
            &[3, 4, 5, 6],
            Check::On,
            Some("circle-check"),
            Some("circle-check"),
        ),
    ];
    for (viewed, check, rest, hovered) in cases {
        set_viewed(&mut o, viewed);
        let state = o.tree.read_with(o.shell.cx, |t, _| t.folder_check("src"));
        assert_eq!(state, Some(check), "{viewed:?}");
        unhover(o.shell.cx);
        assert_eq!(slot_glyph(&mut o, "d:src"), rest, "{viewed:?} at rest");
        hover(o.shell.cx, "tree-name-d:src");
        assert_eq!(slot_glyph(&mut o, "d:src"), hovered, "{viewed:?} hovered");
        // The folder's icon stays `folder` throughout.
        assert_eq!(row_icon(&mut o, "d:src"), Some("folder"), "{viewed:?}");
    }
}

#[gpui_kit::test]
fn filter_field_holds_the_filter_menu_where_the_header_was(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    // No "FILES n" header: the filter field is the segment's first row.
    assert!(painted(o.shell.cx, "tree-count").is_none());
    let pane = bounds(o.shell.cx, "file-tree-pane");
    let field = bounds(o.shell.cx, "tree-filter");
    assert!(field.top() - pane.top() < px(12.), "{field:?} in {pane:?}");
    // The filter menu sits inside the field, at its right edge.
    let menu = bounds(o.shell.cx, "tree-filters");
    assert!(field.contains(&menu.center()), "{menu:?} in {field:?}");
    assert!(menu.left() > field.center().x);
    assert!(
        field.right() - menu.right() <= px(10.),
        "{menu:?} in {field:?}"
    );
    // The rows start below it.
    assert!(bounds(o.shell.cx, "tree-row-f:0").top() >= field.bottom());
    // A click on the menu opens it (not the field's text): its first item
    // is Unviewed.
    click(&mut o, "tree-filters");
    o.shell.cx.simulate_keystrokes("down enter");
    draw(o.shell.cx);
    assert!(o.tree.read_with(o.shell.cx, |t, _| t.filters().unviewed));
}
