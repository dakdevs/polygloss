//! Sticky headers, gap expansion, collapse and special files (T2.5, design
//! §11.6, §6.4, §12.3).

use gpui_kit::{TestAppContext, VisualTestContext};
use polygloss_diff::rows::{ExpandBy, GapId};
use polygloss_diff::{FileKind, Mode, ObjectFormat, Oid, Side};
use polygloss_viewport::{
    BodyRow, ControlAction, FileFlags, HeaderDebug, LayoutMode, RowKey, ScrollTarget, ViewportEvent,
};

use crate::support::*;

/// Two added files: `a.rs` (30 lines, 640 px with its header) and `b.rs` (20
/// lines), unified.
fn two_added() -> std::sync::Arc<MemProvider> {
    MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 30).concat()),
        Spec::added("b.rs", &numbered("b", 20).concat()),
    ])
}

fn header(d: &polygloss_viewport::ViewportDebug, file_idx: u32) -> &HeaderDebug {
    d.headers
        .iter()
        .find(|h| h.file_idx == file_idx)
        .unwrap_or_else(|| panic!("no header for file {file_idx}: {:?}", d.headers))
}

/// Whether the open menu's item `label` is enabled.
fn menu_enabled(d: &polygloss_viewport::ViewportDebug, label: &str) -> bool {
    let menu = d.menu.as_ref().expect("the menu is open");
    menu.items
        .iter()
        .find(|(l, _)| l == label)
        .unwrap_or_else(|| panic!("no menu item {label:?} in {menu:?}"))
        .1
}

fn events_of(events: &std::rc::Rc<std::cell::RefCell<Vec<ViewportEvent>>>) -> Vec<ViewportEvent> {
    events
        .borrow()
        .iter()
        .filter(|e| !matches!(e, ViewportEvent::FrameStats(_)))
        .cloned()
        .collect()
}

#[gpui_kit::test]
fn sticky_header_pins_while_file_scrolls(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, two_added(), opts, 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).y, 0.0);
    assert!(!header(&d, 0).sticky);

    // 300 px into a.rs: its header stays at the top, over the rows it hides.
    wheel(cx, 300.);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[0], "== a.rs");
    assert_eq!(d.row_bounds[0], (0.0, HEADER_H));
    assert_eq!(d.visible_rows[1], unified(None, Some(14), '+', "a 13"));
    assert_eq!(d.row_bounds[1], (0.0, ROW_H));
    let h = header(&d, 0);
    assert!(h.sticky, "{h:?}");
    assert_eq!((h.y, h.title.as_str()), (0.0, "a.rs"));
    // b.rs's header is in place below a.rs's last row.
    assert_eq!(d.headers.len(), 2, "{:?}", d.headers);
    assert_eq!((header(&d, 1).y, header(&d, 1).sticky), (340.0, false));
    // The title is painted on the pinned header, vertically centered.
    assert!(
        d.painted_text
            .iter()
            .any(|(_, y, t)| t == "a.rs" && *y == (HEADER_H - ROW_H) / 2.0),
        "{:?}",
        d.painted_text
    );
    // Its controls moved with it.
    let (_, y, _, hh) = control(&d, ControlAction::Collapse(0));
    assert_eq!((y, hh), (0.0, HEADER_H));

    // The header is painted over the rows: its background comes after the
    // row backgrounds it covers.
    let all = quads(cx);
    let header_bg = all
        .iter()
        .position(|q| q.4 == theme.header_background && q.1 == 0.0 && q.3 == HEADER_H)
        .expect("a header background at the top");
    let last_row_bg = all
        .iter()
        .rposition(|q| q.4 == theme.added_background)
        .unwrap();
    assert!(header_bg > last_row_bg, "{header_bg} {last_row_bg}");

    // Further down it stays pinned.
    wheel(cx, 100.);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).y, 0.0);
    assert_eq!(d.visible_rows[1], unified(None, Some(19), '+', "a 18"));
}

#[gpui_kit::test]
fn sticky_header_pushed_by_next_header(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_added(), options(LayoutMode::Unified), 1000., 400.);

    // a.rs ends 30 px below the top: b.rs's header pushes a.rs's up by 10.
    wheel(cx, 610.);
    let d = debug(&view, cx);
    let (a, b) = (header(&d, 0), header(&d, 1));
    assert!(a.sticky && !b.sticky, "{:?}", d.headers);
    assert_eq!((a.y, b.y), (-10.0, 30.0));
    assert_eq!(d.visible_rows[0], "== a.rs");
    assert_eq!(d.row_bounds[0], (-10.0, HEADER_H));
    let bi = d.visible_rows.iter().position(|r| r == "== b.rs").unwrap();
    assert_eq!(d.row_bounds[bi], (30.0, HEADER_H));

    // At b.rs's top it is in place; one pixel further it pins.
    wheel(cx, 30.);
    let d = debug(&view, cx);
    assert_eq!(d.headers.len(), 1, "{:?}", d.headers);
    assert_eq!((header(&d, 1).y, header(&d, 1).sticky), (0.0, false));
    wheel(cx, 1.);
    let d = debug(&view, cx);
    assert_eq!((header(&d, 1).y, header(&d, 1).sticky), (0.0, true));
    assert_eq!(d.visible_rows[0], "== b.rs");
}

#[gpui_kit::test]
fn sticky_header_blocks_clicks_on_rows_beneath(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        one_hundred_lines(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    let expand = ControlAction::Expand {
        file_idx: 0,
        gap: GapId(1),
        by: ExpandBy::All,
    };
    // Scroll the long gap's row under the pinned header: its controls are
    // still listed (the row is painted) but the header takes the click.
    let (_, gap_y, _, _) = control(&debug(&view, cx), expand);
    wheel(cx, gap_y - 10.);
    let d = debug(&view, cx);
    assert!(header(&d, 0).sticky);
    let (x, y, w, h) = control(&d, expand);
    assert!(y + h / 2.0 < HEADER_H, "{y}");
    click_at(cx, x + w / 2.0, y + h / 2.0);
    assert!(view.read_with(cx, |v, _| v.expansions()).is_empty());
}

/// One file of 100 lines with lines 5 and 90 changed: gap 0 hides old lines
/// 0..2, gap 1 hides 9..87 (78 lines), gap 2 hides 94..100. A 40-line file
/// follows it, so scrolling through it never reaches the document's end.
fn one_hundred_lines() -> std::sync::Arc<MemProvider> {
    let old = numbered("line", 100);
    let mut new = old.clone();
    new[5] = "LINE 5\n".to_owned();
    new[90] = "LINE 90\n".to_owned();
    MemProvider::new(vec![
        Spec::modified("src/long.rs", &old.concat(), &new.concat()),
        Spec::added("src/tail.rs", &numbered("tail", 40).concat()),
    ])
}

fn gap_row(
    view: &gpui_kit::Entity<polygloss_viewport::DiffViewport>,
    cx: &mut VisualTestContext,
    old_line: u32,
) -> Option<BodyRow> {
    view.read_with(cx, |v, _| {
        let layout = v.document().file_layout(0).unwrap();
        let i = layout.find(RowKey::Line {
            side: Side::Old,
            line: old_line,
        })?;
        Some(layout.rows()[i])
    })
}

#[gpui_kit::test]
fn gap_expand_up_20_keeps_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        one_hundred_lines(),
        options(LayoutMode::Unified),
        1000.,
        200.,
    );

    // The gap is above the viewport and the anchor below it: expanding the
    // gap moves nothing on screen.
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 88,
            },
            cx,
        )
    });
    settle(cx);
    let before = debug(&view, cx);
    view.update(cx, |v, cx| v.expand(0, GapId(1), ExpandBy::Up(20), cx));
    settle(cx);
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows, before.visible_rows);
    assert_eq!(after.row_bounds, before.row_bounds);
    // The bottom 20 lines of the gap are revealed; the rest stays hidden.
    assert!(matches!(gap_row(&view, cx, 70), Some(BodyRow::Line { .. })));
    assert!(matches!(
        gap_row(&view, cx, 50),
        Some(BodyRow::Gap {
            old_start: 9,
            len: 58,
            ..
        })
    ));
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[67, 87]])]
    );

    // With the gap on screen below the anchor, clicking "↑ 20" keeps every
    // row above the gap still and shows the revealed lines right above the
    // hunk that follows.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(600.)));
    settle(cx);
    let before = debug(&view, cx);
    let k = before
        .visible_rows
        .iter()
        .position(|r| r == "⋯ 58 unchanged lines")
        .unwrap();
    click_control(
        &view,
        cx,
        ControlAction::Expand {
            file_idx: 0,
            gap: GapId(1),
            by: ExpandBy::Up(20),
        },
    );
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows[..k], before.visible_rows[..k]);
    assert_eq!(after.row_bounds[..=k], before.row_bounds[..=k]);
    assert_eq!(after.visible_rows[k], "⋯ 38 unchanged lines");
    assert_eq!(
        after.visible_rows[k + 1],
        unified(Some(48), Some(48), ' ', "line 47")
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[47, 87]])]
    );
}

#[gpui_kit::test]
fn gap_expand_all_and_expand_file(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        one_hundred_lines(),
        options(LayoutMode::Split),
        1300.,
        1000.,
    );
    let d = debug(&view, cx);
    let gap = |gap, by| ControlAction::Expand {
        file_idx: 0,
        gap: GapId(gap),
        by,
    };
    let has = |d: &polygloss_viewport::ViewportDebug, a| d.controls.iter().any(|c| c.action == a);
    // A gap of 20 lines or fewer offers "Expand all" only; the first gap has
    // nothing above it to expand down from, the last nothing below.
    assert!(has(&d, gap(0, ExpandBy::All)));
    assert!(!has(&d, gap(0, ExpandBy::Up(20))) && !has(&d, gap(0, ExpandBy::Down(20))));
    assert!(has(&d, gap(1, ExpandBy::Up(20))));
    assert!(has(&d, gap(1, ExpandBy::Down(20))));
    assert!(has(&d, gap(1, ExpandBy::All)));
    assert!(d.visible_rows.contains(&"⋯ 78 unchanged lines".to_owned()));

    // "↓ 20" reveals the top of the gap, below the hunk above it.
    click_control(&view, cx, gap(1, ExpandBy::Down(20)));
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[9, 29]])]
    );
    let d = debug(&view, cx);
    assert!(
        d.visible_rows.contains(&"⋯ 58 unchanged lines".to_owned()),
        "{:?}",
        d.visible_rows
    );
    assert!(matches!(
        gap_row(&view, cx, 50),
        Some(BodyRow::Gap {
            old_start: 29,
            len: 58,
            ..
        })
    ));
    let ctx = |n: u32| {
        split(
            Some((n, ' ', &format!("line {}", n - 1))),
            Some((n, ' ', &format!("line {}", n - 1))),
        )
    };
    assert!(d.visible_rows.contains(&ctx(29)), "{:?}", d.visible_rows);

    // "Expand all" reveals the whole gap: its row is gone.
    click_control(&view, cx, gap(1, ExpandBy::All));
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[9, 87]])]
    );
    let d = debug(&view, cx);
    assert!(!d.visible_rows.iter().any(|r| r.contains("58 unchanged")));
    assert!(!has(&d, gap(1, ExpandBy::All)));
    assert!(gap_row(&view, cx, 40).is_some_and(|r| matches!(r, BodyRow::Line { .. })));

    // The whole file: no gap rows left anywhere.
    view.update(cx, |v, cx| v.expand_file(0, cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[0, 100]])]
    );
    let rows = view.read_with(cx, |v, _| {
        v.document().file_layout(0).unwrap().rows().to_vec()
    });
    assert!(
        rows.iter().all(|r| !matches!(r, BodyRow::Gap { .. })),
        "{rows:?}"
    );
    // 98 context rows and the two changed lines (split pairs each).
    assert_eq!(rows.len(), 100);
    assert_eq!(debug(&view, cx).visible_rows[1], ctx(1));
}

#[gpui_kit::test]
fn expansions_survive_layout_toggle_and_restore_with_set_expansions(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        one_hundred_lines(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    view.update(cx, |v, cx| v.set_expansions(0, &[[40, 50], [10, 12]], cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[10, 12], [40, 50]])]
    );
    let revealed = |cx: &mut VisualTestContext| {
        [11, 45, 30].map(|line| matches!(gap_row(&view, cx, line), Some(BodyRow::Line { .. })))
    };
    assert_eq!(revealed(cx), [true, true, false]);
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    assert_eq!(revealed(cx), [true, true, false]);
    // A diff option change recomputes the diff; the same old lines stay
    // revealed.
    set_options(&view, cx, |o| o.diff.ignore_whitespace = true);
    assert_eq!(revealed(cx), [true, true, false]);

    view.update(cx, |v, cx| v.set_expansions(0, &[], cx));
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.expansions()).is_empty());
    assert_eq!(revealed(cx), [false, false, false]);
}

#[gpui_kit::test]
fn collapse_toggle_keeps_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 30).concat()),
        Spec::added("b.rs", &numbered("b", 30).concat()),
        Spec::added("c.rs", &numbered("c", 30).concat()),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 1,
                side: Side::New,
                line: 10,
            },
            cx,
        )
    });
    settle(cx);
    let before = debug(&view, cx);

    // Collapsing a file above the viewport moves nothing on screen.
    view.update(cx, |v, cx| v.set_collapsed(0, true, cx));
    settle(cx);
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows, before.visible_rows);
    assert_eq!(after.row_bounds, before.row_bounds);
    assert_eq!(view.read_with(cx, |v, _| v.collapsed()), vec![0]);

    // Collapsing the file at the top leaves its header there, the next file
    // right below it.
    click_control(&view, cx, ControlAction::Collapse(1));
    let d = debug(&view, cx);
    assert_eq!(view.read_with(cx, |v, _| v.collapsed()), vec![0, 1]);
    assert_eq!(d.anchor.file_idx, 1);
    assert_eq!(d.anchor.row, RowKey::Header);
    assert_eq!(
        d.visible_rows[..3],
        [
            "== b.rs".to_owned(),
            "== c.rs".to_owned(),
            unified(None, Some(1), '+', "c 0")
        ]
    );
    assert_eq!(d.row_bounds[1].0, HEADER_H);
    assert!(header(&d, 1).collapsed && !header(&d, 1).sticky);

    // Expanding it again keeps its header at the top, its rows below.
    click_control(&view, cx, ControlAction::Collapse(1));
    let d = debug(&view, cx);
    assert_eq!(view.read_with(cx, |v, _| v.collapsed()), vec![0]);
    assert_eq!(
        d.visible_rows[..2],
        ["== b.rs".to_owned(), unified(None, Some(1), '+', "b 0")]
    );
    assert_eq!(d.row_bounds[0], (0.0, HEADER_H));

    // Collapsing a file below the anchor keeps everything above it.
    let before = debug(&view, cx);
    view.update(cx, |v, cx| v.set_collapsed(2, true, cx));
    settle(cx);
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows[..5], before.visible_rows[..5]);
}

#[gpui_kit::test]
fn large_file_over_threshold_shows_load_diff(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let old = numbered("line", 10);
    let new = numbered("LINE", 10);
    let provider = MemProvider::new(vec![Spec::modified("big.rs", &old.concat(), &new.concat())]);
    let mut opts = options(LayoutMode::Unified);
    opts.large_file_changed_lines = 5;
    let (view, cx) = open(cx, provider, opts, 1000., 800.);
    let (events, _sub) = record_events(&view, cx);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        ["== big.rs", "Large diff · 20 changed lines"]
    );
    // "Load diff" sits in the placeholder row and is painted.
    let (_, y, _, h) = control(&d, ControlAction::LoadDiff(0));
    let (top, height) = d.row_bounds[1];
    assert!(y >= top && y + h <= top + height, "{y} {h} {top} {height}");
    assert!(d.painted_text.iter().any(|(_, _, t)| t == "Load diff"));
    assert_eq!(header(&d, 0).counts, Some((10, 10)));

    click_control(&view, cx, ControlAction::LoadDiff(0));
    assert_eq!(events_of(&events), [ViewportEvent::LoadDiffRequested(0)]);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows.len(), 21, "{:?}", d.visible_rows);
    assert_eq!(d.visible_rows[1], unified(Some(1), None, '-', "line 0"));
    assert!(
        !d.controls
            .iter()
            .any(|c| c.action == ControlAction::LoadDiff(0))
    );
    assert_eq!(d.anchor.row, RowKey::Header);
}

#[gpui_kit::test]
fn generated_file_collapsed_with_load_diff(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new_with(
        vec![
            Spec::modified("Cargo.lock", "a = 1\n", "a = 2\n"),
            Spec::modified("src/a.rs", "x\n", "y\n"),
        ],
        |files| files[0].generated = true,
    );
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows[..3],
        ["== Cargo.lock", "Generated file", "== src/a.rs"]
    );
    assert!(
        header(&d, 0).badges.contains(&"generated".to_owned()),
        "{:?}",
        d.headers
    );
    assert_eq!(d.row_bounds[1], (HEADER_H, PLACEHOLDER_H));
    control(&d, ControlAction::LoadDiff(0));
    // Only src/a.rs was read.
    assert_eq!(provider.load_count(), 2);

    view.update(cx, |v, cx| v.load_diff(0, cx));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows[..4],
        [
            "== Cargo.lock".to_owned(),
            unified(Some(1), None, '-', "a = 1"),
            unified(None, Some(1), '+', "a = 2"),
            "== src/a.rs".to_owned()
        ]
    );
    assert_eq!(provider.load_count(), 4);
    assert!(header(&d, 0).badges.contains(&"generated".to_owned()));
    assert_eq!(header(&d, 0).counts, Some((1, 1)));
}

/// `n` bytes of binary content (a NUL then filler).
fn bin(n: usize) -> String {
    format!("\0{}", "x".repeat(n - 1))
}

#[gpui_kit::test]
fn binary_placeholder_shows_sizes(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let spec = |path: &str, old: Option<String>, new: Option<String>| Spec {
        path: path.to_owned(),
        old,
        new,
        kind: FileKind::Binary,
        old_path: None,
    };
    let provider = MemProvider::new(vec![
        spec("img.png", Some(bin(12 * 1024)), Some(bin(14_541))),
        spec("new.png", None, Some(bin(900))),
        spec("gone.bin", Some(bin(3 * 1024 * 1024 + 300_000)), None),
    ]);
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        [
            "== img.png",
            "Binary file · 12.0 KB → 14.2 KB",
            "== new.png",
            "Binary file · 900 B",
            "== gone.bin",
            "Binary file · 3.3 MB",
        ]
    );
    assert!(header(&d, 0).badges.contains(&"binary".to_owned()));
    assert_eq!(d.row_bounds[1], (HEADER_H, PLACEHOLDER_H));
    // Sizes come from `blob_size`; no blob is read.
    assert_eq!(provider.load_count(), 0);
}

#[gpui_kit::test]
fn submodule_one_line(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let oid = |prefix: &str| {
        Oid::parse(
            &format!("{prefix}{}", "0".repeat(40 - prefix.len())),
            ObjectFormat::Sha1,
        )
        .unwrap()
    };
    let provider = MemProvider::new_with(
        vec![
            Spec::modified("vendor/lib", "x", "y"),
            Spec::modified("next.rs", "a\n", "b\n"),
        ],
        |files| {
            let f = &mut files[0];
            f.kind = FileKind::Submodule;
            (f.old_mode, f.new_mode) = (Some(Mode::SUBMODULE), Some(Mode::SUBMODULE));
            (f.old_blob, f.new_blob) = (oid("abc1234"), oid("def5678"));
        },
    );
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows[..3],
        ["== vendor/lib", "abc1234 → def5678", "== next.rs"]
    );
    assert_eq!(d.row_bounds[1], (HEADER_H, ROW_H));
    assert_eq!(d.row_bounds[2].0, HEADER_H + ROW_H);
    assert!(header(&d, 0).badges.contains(&"submodule".to_owned()));
    assert_eq!(provider.load_count(), 2);
}

#[gpui_kit::test]
fn mode_only_change_badge_no_body(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new_with(
        vec![
            Spec::modified("run.sh", "echo hi\n", "echo hi\n"),
            Spec::modified("next.rs", "a\n", "b\n"),
        ],
        |files| {
            let f = &mut files[0];
            f.new_blob = f.old_blob.clone();
            f.new_mode = Some(Mode(0o100755));
        },
    );
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[..2], ["== run.sh", "== next.rs"]);
    assert_eq!(d.row_bounds[1].0, HEADER_H);
    assert_eq!(header(&d, 0).badges, ["100644 → 100755"]);
    assert_eq!(header(&d, 0).counts, None);
    assert!(header(&d, 1).badges.is_empty(), "{:?}", header(&d, 1));
    assert_eq!(provider.load_count(), 2);
}

#[gpui_kit::test]
fn rename_header_old_to_new(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let mut spec = Spec::modified("src/new_name.rs", "a\nb\nc\n", "a\nB\nc\n");
    spec.old_path = Some("src/old_name.rs".to_owned());
    let (view, cx) = open(
        cx,
        MemProvider::new(vec![spec]),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[0], "== src/old_name.rs → src/new_name.rs");
    let h = header(&d, 0);
    assert_eq!(h.title, "src/old_name.rs → src/new_name.rs");
    assert_eq!(h.badges, ["90% similar"]);
    assert_eq!(h.counts, Some((1, 1)));
    // The body diffs the two blobs.
    assert!(d.visible_rows.contains(&unified(None, Some(2), '+', "B")));
}

/// A git-lfs pointer file for an object of `size` bytes.
fn lfs_pointer(oid_digit: char, size: u32) -> String {
    format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {size}\n",
        oid_digit.to_string().repeat(64)
    )
}

#[gpui_kit::test]
fn lfs_pointer_badge(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::modified(
            "model.bin",
            &lfs_pointer('a', 1000),
            &lfs_pointer('b', 2000),
        ),
        Spec::modified("notes.txt", "version 1\n", "version 2\n"),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).badges, ["LFS"]);
    assert!(header(&d, 1).badges.is_empty());
    // The pointer is shown as text.
    assert!(
        d.visible_rows
            .contains(&unified(None, Some(3), '+', "size 2000")),
        "{:?}",
        d.visible_rows
    );
}

#[gpui_kit::test]
fn header_menu_comment_on_file_emits_event(cx: &mut TestAppContext) {
    let _sb = sandbox();
    init_kit(cx);
    let (view, cx) = open(cx, two_added(), options(LayoutMode::Unified), 1000., 800.);
    let (events, _sub) = record_events(&view, cx);
    click_control(&view, cx, ControlAction::Menu(1));
    let menu = debug(&view, cx).menu.expect("the menu is open");
    assert_eq!(menu.file_idx, 1);
    let labels: Vec<&str> = menu.items.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Open in editor",
            "Comment on file",
            "Copy path",
            "Expand all",
            "Load diff"
        ]
    );
    click_menu_item(cx, "Comment on file");
    assert_eq!(events_of(&events), [ViewportEvent::FileCommentRequested(1)]);
    assert!(debug(&view, cx).menu.is_none());
}

#[gpui_kit::test]
fn header_menu_open_in_editor_copy_path_expand_all_and_load_diff(cx: &mut TestAppContext) {
    let _sb = sandbox();
    init_kit(cx);
    let old = numbered("line", 100);
    let mut new = old.clone();
    new[50] = "LINE 50\n".to_owned();
    let provider = MemProvider::new_with(
        vec![
            Spec::modified("src/mid.rs", &old.concat(), &new.concat()),
            Spec::modified("yarn.lock", "a\n", "b\n"),
        ],
        |files| files[1].generated = true,
    );
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 800.);
    let (events, _sub) = record_events(&view, cx);

    // Open in editor: the new side at the file's first change.
    click_control(&view, cx, ControlAction::Menu(0));
    let d = debug(&view, cx);
    assert!(!menu_enabled(&d, "Load diff") && menu_enabled(&d, "Expand all"));
    click_menu_item(cx, "Open in editor");
    assert_eq!(
        events_of(&events),
        [ViewportEvent::OpenInEditor {
            file_idx: 0,
            side: Side::New,
            line: 50
        }]
    );

    click_control(&view, cx, ControlAction::Menu(0));
    click_menu_item(cx, "Copy path");
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("src/mid.rs"));

    click_control(&view, cx, ControlAction::Menu(0));
    click_menu_item(cx, "Expand all");
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[0, 100]])]
    );

    // The generated file's menu can load it.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(1), cx));
    settle(cx);
    click_control(&view, cx, ControlAction::Menu(1));
    let d = debug(&view, cx);
    assert!(menu_enabled(&d, "Load diff") && !menu_enabled(&d, "Expand all"));
    click_menu_item(cx, "Load diff");
    let d = debug(&view, cx);
    let at = d
        .visible_rows
        .iter()
        .position(|r| r == "== yarn.lock")
        .unwrap();
    assert_eq!(d.visible_rows[at + 1], unified(Some(1), None, '-', "a"));
    assert!(events_of(&events).contains(&ViewportEvent::LoadDiffRequested(1)));
}

#[gpui_kit::test]
fn viewed_checkbox_emits_event_and_flags_render(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, two_added(), options(LayoutMode::Unified), 1000., 800.);
    let (events, _sub) = record_events(&view, cx);

    // The viewport only reports the click; Viewed is the host's state.
    click_control(&view, cx, ControlAction::Viewed(1));
    assert_eq!(events_of(&events), [ViewportEvent::ViewedToggled(1)]);
    assert!(!header(&debug(&view, cx), 1).viewed);

    view.update(cx, |v, cx| {
        v.set_file_flags(
            vec![
                FileFlags {
                    changed_since_viewed: true,
                    open_threads: 3,
                    agent_threads: true,
                    ..FileFlags::default()
                },
                FileFlags {
                    viewed: true,
                    open_threads: 1,
                    ..FileFlags::default()
                },
            ],
            cx,
        )
    });
    settle(cx);
    let d = debug(&view, cx);
    let (a, b) = (header(&d, 0), header(&d, 1));
    assert!(!a.viewed && b.viewed);
    assert_eq!(
        a.badges,
        ["changed since viewed", "3 open threads", "agent"]
    );
    assert_eq!(b.badges, ["1 open thread"]);
    // Painted: the "Viewed" label on both headers, a checkmark on b.rs's.
    let viewed_labels = d
        .painted_text
        .iter()
        .filter(|(_, _, t)| t == "Viewed")
        .count();
    assert_eq!(viewed_labels, 2);
    let (x, y, w, h) = control(&d, ControlAction::Viewed(1));
    assert!(
        d.painted_text.iter().any(|(tx, ty, t)| t == "✓"
            && *tx >= x
            && *tx < x + w
            && *ty >= y
            && *ty < y + h),
        "{:?}",
        d.painted_text
    );
}

#[gpui_kit::test]
fn header_counts_fill_in_once_known(cx: &mut TestAppContext) {
    use gpui_kit::{px, size};
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::modified("a.rs", "a\nb\nc\n", "a\nB\nc\nd\n")]);
    let opts = options(LayoutMode::Unified);
    let window = cx.open_window(size(px(1000.), px(400.)), move |window, cx| {
        polygloss_viewport::DiffViewport::new(provider, opts, window, cx)
    });
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    redraw(cx);
    assert_eq!(header(&debug(&view, cx), 0).counts, None);
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).counts, Some((2, 1)));
    let texts: Vec<&str> = d.painted_text.iter().map(|(_, _, t)| t.as_str()).collect();
    assert!(texts.contains(&"+2") && texts.contains(&"−1"), "{texts:?}");
}

#[gpui_kit::test]
fn hovering_a_control_highlights_it(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, point, px};
    let _sb = sandbox();
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, one_hundred_lines(), opts, 1000., 600.);
    let hover_at = |cx: &mut VisualTestContext, action: Option<ControlAction>| {
        let (x, y) = match action {
            Some(action) => {
                let (x, y, w, h) = control(&debug(&view, cx), action);
                (x + w / 2.0, y + h / 2.0)
            }
            // The middle of a code row.
            None => (500.0, 150.0),
        };
        cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
        settle(cx);
        quads_of(cx, theme.hover)
    };
    let close = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
        [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)]
            .iter()
            .all(|(p, q)| (p - q).abs() <= 0.5 + 1e-3)
    };
    // Over a code row nothing is lit (the pointer starts at the window's
    // corner, over the first header's chevron).
    assert!(hover_at(cx, None).is_empty());
    let all = ControlAction::Expand {
        file_idx: 0,
        gap: GapId(1),
        by: ExpandBy::All,
    };
    let lit = hover_at(cx, Some(all));
    assert_eq!(lit.len(), 1, "{lit:?}");
    assert!(close(lit[0], control(&debug(&view, cx), all)), "{lit:?}");
    // Header controls light up too, and only one control at a time.
    let lit = hover_at(cx, Some(ControlAction::Collapse(0)));
    assert_eq!(lit.len(), 1, "{lit:?}");
    assert!(close(
        lit[0],
        control(&debug(&view, cx), ControlAction::Collapse(0))
    ));
    assert!(hover_at(cx, None).is_empty());
}

#[gpui_kit::test]
fn press_and_release_on_different_controls_does_nothing(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, point, px};
    let _sb = sandbox();
    let (view, cx) = open(cx, two_added(), options(LayoutMode::Unified), 1000., 800.);
    let (events, _sub) = record_events(&view, cx);
    let d = debug(&view, cx);
    let center = |(x, y, w, h): (f32, f32, f32, f32)| point(px(x + w / 2.0), px(y + h / 2.0));
    let chevron = center(control(&d, ControlAction::Collapse(0)));
    let viewed = center(control(&d, ControlAction::Viewed(0)));
    cx.simulate_mouse_move(chevron, None, Modifiers::default());
    cx.simulate_mouse_down(chevron, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(viewed, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(viewed, MouseButton::Left, Modifiers::default());
    settle(cx);
    assert!(events_of(&events).is_empty());
    assert!(view.read_with(cx, |v, _| v.collapsed()).is_empty());
}

#[gpui_kit::test]
fn header_fits_narrow_widths(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let path = "src/some/deeply/nested/directory/component_name.rs";
    let provider = MemProvider::new_with(vec![Spec::modified(path, "a\n", "b\n")], |files| {
        files[0].generated = true;
        files[0].new_mode = Some(Mode(0o100755));
    });
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 400., 300.);
    view.update(cx, |v, cx| {
        v.set_file_flags(
            vec![FileFlags {
                open_threads: 3,
                ..FileFlags::default()
            }],
            cx,
        )
    });
    settle(cx);
    let d = debug(&view, cx);
    let h = header(&d, 0);
    // The change badges give way first; the review flag stays.
    assert_eq!(h.badges, ["3 open threads"]);
    // The title keeps its end, cut with "…", and does not run into the
    // badge.
    assert!(h.title.starts_with('…'), "{h:?}");
    assert!(path.ends_with(h.title.trim_start_matches('…')), "{h:?}");
    assert!(h.title.chars().count() >= 12, "{h:?}");
    let x_of = |text: &str| {
        d.painted_text
            .iter()
            .find(|(_, _, t)| t == text)
            .map(|(x, _, _)| *x)
            .unwrap_or_else(|| panic!("{text:?} not painted: {:?}", d.painted_text))
    };
    let title_end = x_of(&h.title) + h.title.chars().count() as f32 * ADVANCE;
    assert!(title_end <= x_of("3 open threads"), "{title_end}");
    // Every control is still there.
    for action in [
        ControlAction::Collapse(0),
        ControlAction::Viewed(0),
        ControlAction::Menu(0),
    ] {
        let (x, _, w, _) = control(&d, action);
        assert!(x >= 0.0 && x + w <= 400.0 + 1e-3, "{action:?}");
    }
}

#[gpui_kit::test]
fn menu_closes_on_scroll(cx: &mut TestAppContext) {
    let _sb = sandbox();
    init_kit(cx);
    let (view, cx) = open(cx, two_added(), options(LayoutMode::Unified), 1000., 400.);
    click_control(&view, cx, ControlAction::Menu(0));
    assert!(debug(&view, cx).menu.is_some());
    wheel(cx, 20.);
    assert!(debug(&view, cx).menu.is_none());
}

#[gpui_kit::test]
fn expanders_act_on_the_run_they_are_drawn_on(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        one_hundred_lines(),
        options(LayoutMode::Unified),
        1000.,
        1000.,
    );
    // Revealing 40..50 splits gap 1 (9..87) into two hidden runs, 9..40 and
    // 50..87, both rows of `GapId(1)`.
    view.update(cx, |v, cx| v.set_expansions(0, &[[40, 50]], cx));
    settle(cx);
    let d = debug(&view, cx);
    let runs: Vec<&String> = d
        .visible_rows
        .iter()
        .filter(|r| r.starts_with('⋯'))
        .collect();
    assert_eq!(runs[1..3], ["⋯ 31 unchanged lines", "⋯ 37 unchanged lines"]);
    let expand = |by| ControlAction::Expand {
        file_idx: 0,
        gap: GapId(1),
        by,
    };
    // "↑ 20" on the first run reveals the bottom of that run.
    click_control(&view, cx, expand(ExpandBy::Up(20)));
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[20, 50]])]
    );
    // The first run is 11 lines now ("Expand all" only): the first "↓ 20"
    // belongs to the second run and reveals its top.
    click_control(&view, cx, expand(ExpandBy::Down(20)));
    assert_eq!(
        view.read_with(cx, |v, _| v.expansions()),
        vec![(0, vec![[20, 70]])]
    );
}

#[gpui_kit::test]
fn symlink_diffs_its_target_with_a_badge(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let mut spec = Spec::modified("bin/tool", "../old/tool", "../new/tool");
    spec.kind = FileKind::Symlink;
    let provider = MemProvider::new_with(vec![spec], |files| {
        (files[0].old_mode, files[0].new_mode) = (Some(Mode::SYMLINK), Some(Mode::SYMLINK));
    });
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).badges, ["symlink"]);
    // A link target has no trailing newline.
    let no_newline = "\\ No newline at end of file".to_owned();
    assert_eq!(
        d.visible_rows[1..5],
        [
            unified(Some(1), None, '-', "../old/tool"),
            no_newline.clone(),
            unified(None, Some(1), '+', "../new/tool"),
            no_newline
        ]
    );
}

#[test]
fn sizes_and_lfs_pointers_follow_their_rules() {
    use polygloss_viewport::special::{format_size, is_lfs_pointer};
    let cases = [
        (0, "0 B"),
        (1023, "1023 B"),
        (1024, "1.0 KB"),
        (12 * 1024, "12.0 KB"),
        (14_541, "14.2 KB"),
        (1024 * 1024 - 1, "1.0 MB"),
        (3_445_728, "3.3 MB"),
        (5 * 1024 * 1024 * 1024, "5.0 GB"),
    ];
    for (bytes, text) in cases {
        assert_eq!(format_size(bytes), text, "{bytes}");
    }
    let pointer = lfs_pointer('a', 1000);
    assert!(is_lfs_pointer(pointer.as_bytes()));
    let hawser = pointer.replace("git-lfs.github.com", "hawser.github.com");
    assert!(is_lfs_pointer(hawser.as_bytes()));
    // Not pointers: a size that is not a number, a missing oid, a big file
    // that merely starts like one, another first line.
    assert!(!is_lfs_pointer(
        pointer.replace("size 1000", "size lots").as_bytes()
    ));
    assert!(!is_lfs_pointer(pointer.replace("oid ", "id ").as_bytes()));
    assert!(!is_lfs_pointer(
        format!("{pointer}{}", "x".repeat(1024)).as_bytes()
    ));
    assert!(!is_lfs_pointer(format!("# notes\n{pointer}").as_bytes()));
}
