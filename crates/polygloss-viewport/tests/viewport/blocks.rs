//! Variable-height blocks (T2.7, design §11.6 "Threads", §12.4 "Blocks"):
//! host elements below their anchored line, in their side's column with a
//! same-height spacer in split, measured when near the viewport, with the
//! scroll anchor keeping visible content still.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Entity, Hsla, InteractiveElement as _, IntoElement as _, Modifiers, MouseButton, Styled as _,
    TestAppContext, VisualTestContext, div, hsla, point, px,
};
use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::{Expansions, Layout, build_rows};
use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, Mode, ObjectFormat, Oid, Side};
use polygloss_viewport::document::{BodyRow, Document, FileLayout, Metrics, RowKey};
use polygloss_viewport::{
    BlockAnchor, BlockId, BlockSpec, DiffViewport, ESTIMATED_BLOCK_ROWS, LayoutMode, PlacedBlock,
    ScrollTarget,
};

use crate::support::*;

// ---------------------------------------------------------------------------
// helpers

fn old(line: u32) -> BlockAnchor {
    BlockAnchor::Line {
        side: Side::Old,
        line,
    }
}

fn new(line: u32) -> BlockAnchor {
    BlockAnchor::Line {
        side: Side::New,
        line,
    }
}

fn placed(id: u64, anchor: BlockAnchor, height: f32) -> PlacedBlock {
    PlacedBlock {
        id: BlockId(id),
        anchor,
        height,
    }
}

/// The body rows of a diff of `old` → `new` at default metrics.
fn layout_of(old: &str, new: &str, layout: Layout) -> FileLayout {
    let diff = diff_blobs(old.as_bytes(), new.as_bytes(), &DiffOptions::default());
    let rows = build_rows(&diff, &Expansions::default(), layout);
    FileLayout::from_rows(&rows, &Metrics::default())
}

/// Each row of `layout`, compactly: `o7 n7` (a line row, `-` for a missing
/// side), `gap o0+7`, `nl old`, `[3]` (a block) or `placeholder`.
fn describe(layout: &FileLayout) -> Vec<String> {
    let n = |v: Option<u32>| v.map_or("-".to_owned(), |v| v.to_string());
    layout
        .rows()
        .iter()
        .map(|r| match r {
            BodyRow::Line { old, new, .. } => format!("o{} n{}", n(*old), n(*new)),
            BodyRow::Gap { old_start, len, .. } => format!("gap o{old_start}+{len}"),
            BodyRow::NoNewline { side, .. } => format!("nl {side:?}").to_lowercase(),
            BodyRow::Block(id) => format!("[{}]", id.0),
            BodyRow::Placeholder => "placeholder".to_owned(),
        })
        .collect()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

/// 20 numbered lines with line 10 changed: a gap, three context lines, the
/// change, three context lines, a gap.
fn twenty_lines() -> (String, String) {
    let lines = numbered("line", 20);
    let mut changed = lines.clone();
    changed[10] = "LINE 10\n".to_owned();
    (lines.concat(), changed.concat())
}

fn text_change(idx: u32) -> FileChange {
    let oid = |s: &str| Oid::parse(s, ObjectFormat::Sha1).unwrap();
    let p = Some(GitPath::from_bytes(format!("src/f{idx}.rs").as_bytes()));
    FileChange {
        idx,
        status: FileStatus::Modified,
        old_path: p.clone(),
        new_path: p,
        old_mode: Some(Mode(0o100644)),
        new_mode: Some(Mode(0o100644)),
        old_blob: oid("ce013625030ba8dba906f756967f9e9ca394464a"),
        new_blob: oid("5716ca5987cbf97d6bb54920bea6adde242d87e6"),
        similarity: None,
        kind: FileKind::Text,
        generated: false,
    }
}

/// A body of `n` unified context rows (old = new = line), 20 px each.
fn context_layout(n: u32) -> FileLayout {
    let rows = (0..n)
        .map(|i| BodyRow::Line {
            old: Some(i),
            new: Some(i),
            diff_row: i,
        })
        .collect();
    FileLayout::new(rows, &vec![20.0; n as usize])
}

/// A block that is a plain `color` box `height` px tall, as wide as its
/// column.
fn boxed(id: u64, anchor: BlockAnchor, height: f32, color: Hsla) -> BlockSpec {
    BlockSpec {
        id: BlockId(id),
        anchor,
        render: Rc::new(move |_, _| div().w_full().h(px(height)).bg(color).into_any_element()),
    }
}

/// A box whose height the test changes through the returned cell.
fn resizable(id: u64, anchor: BlockAnchor, height: f32, color: Hsla) -> (BlockSpec, Rc<Cell<f32>>) {
    let h = Rc::new(Cell::new(height));
    let cell = h.clone();
    let spec = BlockSpec {
        id: BlockId(id),
        anchor,
        render: Rc::new(move |_, _| div().w_full().h(px(h.get())).bg(color).into_any_element()),
    };
    (spec, cell)
}

fn color(hue: f32) -> Hsla {
    hsla(hue, 0.6, 0.5, 1.0)
}

fn set_blocks(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
    file: u32,
    blocks: Vec<BlockSpec>,
) {
    view.update(cx, |v, cx| v.set_blocks(file, blocks, cx));
    settle(cx);
}

fn layout_rows(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, file: u32) -> Vec<String> {
    view.read_with(cx, |v, _| {
        describe(v.document().file_layout(file).expect("file is laid out"))
    })
}

/// Index of `row` among the visible rows.
fn row_index(rows: &[String], row: &str) -> usize {
    rows.iter()
        .position(|r| r == row)
        .unwrap_or_else(|| panic!("{row:?} not in {rows:#?}"))
}

/// The position of the visible row `row` as `(top, height)`.
fn bounds_of(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, row: &str) -> (f32, f32) {
    let d = debug(view, cx);
    d.row_bounds[row_index(&d.visible_rows, row)]
}

/// Asserts `actual` quads are `expected`, in any order.
fn assert_quads(actual: &[(f32, f32, f32, f32)], expected: &[(f32, f32, f32, f32)]) {
    let sorted = |q: &[(f32, f32, f32, f32)]| {
        let mut q = q.to_vec();
        q.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
        q
    };
    let (actual, expected) = (sorted(actual), sorted(expected));
    // GPUI snaps quad edges to device pixels (2×): allow half a point.
    let close = |a: &(f32, f32, f32, f32), e: &(f32, f32, f32, f32)| {
        (a.0 - e.0).abs() <= 0.5
            && (a.1 - e.1).abs() <= 0.5
            && (a.2 - e.2).abs() <= 0.5
            && (a.3 - e.3).abs() <= 0.5
    };
    let same =
        actual.len() == expected.len() && actual.iter().zip(&expected).all(|(a, e)| close(a, e));
    assert!(same, "quads {actual:?}, expected {expected:?}");
}

// ---------------------------------------------------------------------------
// placement (pure)

#[test]
fn placement_puts_blocks_below_their_line_in_host_order() {
    let (a, b) = twenty_lines();
    let blocks = [
        placed(1, new(10), 30.0),
        placed(2, old(10), 40.0),
        placed(3, BlockAnchor::FileTop, 50.0),
        placed(4, new(12), 60.0),
        // Hidden in the gap above the hunk: below that gap.
        placed(5, old(2), 70.0),
        // Past the end of the file: below the last row of that side.
        placed(6, new(99), 80.0),
        placed(7, new(10), 90.0),
    ];

    let unified = layout_of(&a, &b, Layout::Unified);
    let placed_unified = unified.with_blocks(&blocks);
    assert_eq!(
        describe(&placed_unified),
        strs(&[
            "[3]",
            "gap o0+7",
            "[5]",
            "o7 n7",
            "o8 n8",
            "o9 n9",
            "o10 n-",
            "[2]",
            "o- n10",
            "[1]",
            "[7]",
            "o11 n11",
            "o12 n12",
            "[4]",
            "o13 n13",
            "gap o14+6",
            "[6]",
        ])
    );
    let extra: f32 = blocks.iter().map(|b| b.height).sum();
    assert_eq!(placed_unified.height(), unified.height() + f64::from(extra));
    let block_row = |l: &FileLayout, id: u64| l.find(RowKey::Block(BlockId(id))).unwrap();
    assert_eq!(
        placed_unified.row_height(block_row(&placed_unified, 4)),
        60.0
    );
    assert_eq!(placed_unified.block_rows(), &[0, 2, 7, 9, 10, 13, 16]);

    // Split: a changed pair is one row, so blocks on either of its lines
    // follow it in host order.
    let split = layout_of(&a, &b, Layout::Split).with_blocks(&blocks);
    assert_eq!(
        describe(&split),
        strs(&[
            "[3]",
            "gap o0+7",
            "[5]",
            "o7 n7",
            "o8 n8",
            "o9 n9",
            "o10 n10",
            "[1]",
            "[2]",
            "[7]",
            "o11 n11",
            "o12 n12",
            "[4]",
            "o13 n13",
            "gap o14+6",
            "[6]",
        ])
    );
}

#[test]
fn placement_keeps_no_newline_markers_with_their_line() {
    let layout = layout_of("a\nb", "a\nc", Layout::Unified);
    assert_eq!(
        describe(&layout),
        strs(&["o0 n0", "o1 n-", "nl old", "o- n1", "nl new"])
    );
    let placed = layout.with_blocks(&[placed(1, old(1), 20.0), placed(2, new(1), 20.0)]);
    assert_eq!(
        describe(&placed),
        strs(&["o0 n0", "o1 n-", "nl old", "[1]", "o- n1", "nl new", "[2]"])
    );
}

#[test]
fn placement_on_bodies_without_rows() {
    let blocks = [
        placed(1, new(3), 20.0),
        placed(2, BlockAnchor::FileTop, 30.0),
    ];
    let placeholder = FileLayout::placeholder(48.0).with_blocks(&blocks);
    assert_eq!(describe(&placeholder), strs(&["[2]", "placeholder", "[1]"]));
    assert_eq!(placeholder.height(), 98.0);
    let empty = FileLayout::new(Vec::new(), &[]).with_blocks(&blocks);
    assert_eq!(describe(&empty), strs(&["[2]", "[1]"]));
    // An old-side anchor in a file without old lines (an added file) goes to
    // the end.
    let added = layout_of("", "x\ny\n", Layout::Unified).with_blocks(&[placed(3, old(0), 5.0)]);
    assert_eq!(describe(&added), strs(&["o- n0", "o- n1", "[3]"]));
}

#[test]
fn placement_replaces_earlier_blocks_and_keeps_row_heights() {
    let mut base = context_layout(4);
    base.set_row_height(2, 55.0);
    let first = base.with_blocks(&[placed(1, new(0), 10.0), placed(2, new(2), 10.0)]);
    let second = first.with_blocks(&[placed(3, new(3), 25.0)]);
    assert_eq!(
        describe(&second),
        strs(&["o0 n0", "o1 n1", "o2 n2", "o3 n3", "[3]"])
    );
    assert_eq!(second.row_height(2), 55.0);
    assert_eq!(second.height(), 20.0 * 3.0 + 55.0 + 25.0);
    // No blocks: the rows alone.
    assert_eq!(first.with_blocks(&[]), base);
}

// ---------------------------------------------------------------------------
// document (pure)

#[test]
fn document_blocks_count_in_estimates_layouts_and_explicit_heights() {
    let files = Arc::new((0..3).map(text_change).collect::<Vec<_>>());
    let mut d = Document::new(files, Metrics::default());
    d.set_viewport_height(400.0);
    let estimated = d.file_height(1);

    d.set_blocks(
        1,
        vec![
            placed(1, new(2), 60.0),
            placed(2, BlockAnchor::FileTop, 40.0),
        ],
    );
    assert!(!d.is_exact(1));
    assert_eq!(d.file_height(1), estimated + 100.0);
    assert_eq!(d.blocks(1).len(), 2);

    // A layout handed over later gets the file's blocks.
    d.set_file_layout(1, context_layout(10));
    assert_eq!(
        describe(d.file_layout(1).unwrap()),
        strs(&[
            "[2]", "o0 n0", "o1 n1", "o2 n2", "[1]", "o3 n3", "o4 n4", "o5 n5", "o6 n6", "o7 n7",
            "o8 n8", "o9 n9",
        ])
    );
    assert_eq!(d.file_height(1), 40.0 + 200.0 + 100.0);

    // A measured height is kept by relayouts.
    assert!(d.set_block_height(1, BlockId(1), 75.0));
    assert!(!d.set_block_height(1, BlockId(9), 75.0));
    assert_eq!(d.blocks(1)[0].height, 75.0);
    d.set_file_layout(1, context_layout(10));
    assert_eq!(d.file_height(1), 40.0 + 200.0 + 115.0);
    // So is one set on the block's row directly.
    let row = d
        .file_layout(1)
        .unwrap()
        .find(RowKey::Block(BlockId(2)))
        .unwrap();
    d.set_row_height(1, row as u32, 10.0);
    assert_eq!(d.blocks(1)[1].height, 10.0);

    // A geometry change drops layouts, not blocks.
    d.set_metrics(Metrics {
        row_height: 24.0,
        ..Metrics::default()
    });
    assert_eq!(d.blocks(1).len(), 2);
    d.set_file_layout(1, context_layout(10));
    assert_eq!(d.file_layout(1).unwrap().block_rows().len(), 2);

    // A collapsed file is its header alone, blocks included.
    d.set_collapsed(1, true);
    assert_eq!(d.file_height(1), 40.0);
    d.set_collapsed(1, false);

    // Removing blocks re-lays out the rows alone.
    d.set_blocks(1, Vec::new());
    assert_eq!(d.file_layout(1).unwrap(), &context_layout(10));

    // A row-less exact body grows and shrinks with its blocks.
    d.set_file_height(2, 300.0);
    d.set_blocks(2, vec![placed(3, new(0), 50.0)]);
    assert_eq!(d.file_height(2), 350.0);
    d.set_blocks(2, Vec::new());
    assert_eq!(d.file_height(2), 300.0);
}

#[test]
fn document_block_changes_keep_the_anchor() {
    let files = Arc::new((0..2).map(text_change).collect::<Vec<_>>());
    let mut d = Document::new(files, Metrics::default());
    d.set_viewport_height(200.0);
    d.set_file_layout(0, context_layout(40));
    d.set_file_layout(1, context_layout(40));
    d.scroll_to(
        0,
        RowKey::Line {
            side: Side::New,
            line: 20,
        },
    );
    let anchor = *d.anchor();
    let top = d.scroll_top();

    // Blocks above the anchor: the content on screen does not move.
    d.set_blocks(
        0,
        vec![
            placed(1, new(3), 80.0),
            placed(2, BlockAnchor::FileTop, 10.0),
        ],
    );
    assert_eq!(*d.anchor(), anchor);
    assert_eq!(d.scroll_top(), top + 90.0);
    d.set_block_height(0, BlockId(1), 30.0);
    assert_eq!(*d.anchor(), anchor);
    assert_eq!(d.scroll_top(), top + 40.0);
    // Duplicate ids keep the first.
    d.set_blocks(1, vec![placed(5, new(1), 10.0), placed(5, new(2), 99.0)]);
    assert_eq!(d.blocks(1), &[placed(5, new(1), 10.0)]);
}

#[test]
fn document_scroll_to_block_lands_once_its_file_is_laid_out() {
    let files = Arc::new((0..6).map(text_change).collect::<Vec<_>>());
    let mut d = Document::new(files, Metrics::default());
    d.set_viewport_height(200.0);
    d.set_blocks(4, vec![placed(7, new(12), 50.0)]);
    d.scroll_to(4, RowKey::Block(BlockId(7)));
    assert_eq!(d.anchor().row, RowKey::Block(BlockId(7)));
    // Estimated: below its line, spread at the row height.
    assert_eq!(d.scroll_top(), d.file_top(4) + 40.0 + 13.0 * 20.0);
    d.set_file_layout(4, context_layout(30));
    let row = d
        .file_layout(4)
        .unwrap()
        .find(RowKey::Block(BlockId(7)))
        .unwrap();
    assert_eq!(row, 13);
    assert_eq!(d.scroll_top(), d.file_top(4) + 40.0 + 13.0 * 20.0);
    assert_eq!(d.anchor().row, RowKey::Block(BlockId(7)));
}

// ---------------------------------------------------------------------------
// viewport (GPUI)

/// Ten numbered lines, line 4 changed (the T2.4 `one_change` file): a 1-line
/// gap, context 1–3, the change, context 5–7, a 2-line gap.
fn one_change() -> Spec {
    let old = numbered("line", 10);
    let mut new = old.clone();
    new[4] = "LINE 4\n".to_owned();
    Spec::modified("src/a.rs", &old.concat(), &new.concat())
}

#[gpui_kit::test]
fn block_below_anchored_line_unified(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let c = color(0.1);
    set_blocks(&view, cx, 0, vec![boxed(7, new(4), 120.0, c)]);

    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        vec![
            "== src/a.rs".to_owned(),
            "⋯ 1 unchanged line".to_owned(),
            unified(Some(2), Some(2), ' ', "line 1"),
            unified(Some(3), Some(3), ' ', "line 2"),
            unified(Some(4), Some(4), ' ', "line 3"),
            unified(Some(5), None, '-', "line 4"),
            unified(None, Some(5), '+', "LINE 4"),
            "[block 7]".to_owned(),
            unified(Some(6), Some(6), ' ', "line 5"),
            unified(Some(7), Some(7), ' ', "line 6"),
            unified(Some(8), Some(8), ' ', "line 7"),
            "⋯ 2 unchanged lines".to_owned(),
        ]
    );
    // Measured, not estimated: the row is as tall as the element.
    let top = HEADER_H + 32.0 + 5.0 * ROW_H;
    assert_eq!(d.row_bounds[7], (top, 120.0));
    assert_eq!(d.row_bounds[8], (top + 120.0, ROW_H));
    // Full width in unified.
    assert_quads(&quads_of(cx, c), &[(0.0, top, 1000.0, 120.0)]);
    view.read_with(cx, |v, _| {
        assert_eq!(v.document().blocks(0), &[placed(7, new(4), 120.0)]);
    });
}

#[gpui_kit::test]
fn block_split_left_column_with_right_spacer(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let opts = options(LayoutMode::Split);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    let (left, right) = (color(0.1), color(0.4));
    set_blocks(
        &view,
        cx,
        0,
        vec![boxed(1, old(4), 120.0, left), boxed(2, new(5), 60.0, right)],
    );

    let d = debug(&view, cx);
    let ctx = |n: u32, t: &str| split(Some((n, ' ', t)), Some((n, ' ', t)));
    assert_eq!(
        d.visible_rows,
        vec![
            "== src/a.rs".to_owned(),
            "⋯ 1 unchanged line".to_owned(),
            ctx(2, "line 1"),
            ctx(3, "line 2"),
            ctx(4, "line 3"),
            split(Some((5, '-', "line 4")), Some((5, '+', "LINE 4"))),
            "[block 1]".to_owned(),
            ctx(6, "line 5"),
            "[block 2]".to_owned(),
            ctx(7, "line 6"),
            ctx(8, "line 7"),
            "⋯ 2 unchanged lines".to_owned(),
        ]
    );
    let y1 = HEADER_H + 32.0 + 4.0 * ROW_H;
    let y2 = y1 + 120.0 + ROW_H;
    assert_eq!(d.row_bounds[6], (y1, 120.0));
    assert_eq!(d.row_bounds[8], (y2, 60.0));
    // Each block in its side's column (the left one stops before the
    // divider), a same-height spacer on the other side.
    assert_quads(&quads_of(cx, left), &[(0.0, y1, 499.0, 120.0)]);
    assert_quads(&quads_of(cx, right), &[(500.0, y2, 500.0, 60.0)]);
    assert_quads(
        &quads_of(cx, theme.empty_cell),
        &[(500.0, y1, 500.0, 120.0), (0.0, y2, 500.0, 60.0)],
    );
    // The divider runs through both block rows.
    let dividers = quads_of(cx, theme.border);
    for y in [y1, y2] {
        assert!(
            dividers
                .iter()
                .any(|q| (q.0 - 499.0).abs() <= 0.5 && (q.1 - y).abs() <= 0.5 && q.2 <= 1.5),
            "no divider at y={y}: {dividers:?}"
        );
    }
}

#[gpui_kit::test]
fn file_top_block_under_header(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change(), Spec::added("src/b.rs", "b\n")]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1000., 600.);
    let (c, c2) = (color(0.2), color(0.7));
    set_blocks(&view, cx, 0, vec![boxed(3, BlockAnchor::FileTop, 50.0, c)]);
    // A file-level block on a file with a single side.
    set_blocks(&view, cx, 1, vec![boxed(4, BlockAnchor::FileTop, 30.0, c2)]);

    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[0], "== src/a.rs");
    assert_eq!(d.visible_rows[1], "[block 3]");
    assert_eq!(d.visible_rows[2], "⋯ 1 unchanged line");
    assert_eq!(d.row_bounds[1], (HEADER_H, 50.0));
    // Full width even in split, and no spacer.
    assert_quads(&quads_of(cx, c), &[(0.0, HEADER_H, 1000.0, 50.0)]);
    let b = row_index(&d.visible_rows, "== src/b.rs");
    assert_eq!(d.visible_rows[b + 1], "[block 4]");
    let (y, h) = d.row_bounds[b + 1];
    assert_eq!(h, 30.0);
    assert_quads(&quads_of(cx, c2), &[(0.0, y, 1000.0, 30.0)]);
}

#[gpui_kit::test]
fn blocks_survive_layout_toggle(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1000., 800.);
    let (l, r, t) = (color(0.1), color(0.4), color(0.8));
    set_blocks(
        &view,
        cx,
        0,
        vec![
            boxed(1, old(4), 40.0, l),
            boxed(2, new(4), 50.0, r),
            boxed(3, BlockAnchor::FileTop, 30.0, t),
        ],
    );
    let split_rows = debug(&view, cx).visible_rows;
    assert_eq!(
        split_rows
            .iter()
            .filter(|r| r.starts_with("[block"))
            .count(),
        3
    );

    set_options(&view, cx, |o| o.layout = LayoutMode::Unified);
    let d = debug(&view, cx);
    let at = |row: &str| row_index(&d.visible_rows, row);
    assert_eq!(at("[block 3]"), 1);
    // Each block below its own line: the removed line, then the added one.
    assert_eq!(
        at("[block 1]"),
        at(&unified(Some(5), None, '-', "line 4")) + 1
    );
    assert_eq!(
        at("[block 2]"),
        at(&unified(None, Some(5), '+', "LINE 4")) + 1
    );
    for c in [l, r, t] {
        let q = quads_of(cx, c);
        assert_eq!(q.len(), 1, "one full-width box per block");
        assert!((q[0].2 - 1000.0).abs() <= 0.5, "{q:?}");
    }

    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    assert_eq!(debug(&view, cx).visible_rows, split_rows);
    let (y1, _) = bounds_of(&view, cx, "[block 1]");
    let (y2, _) = bounds_of(&view, cx, "[block 2]");
    // The left-side block is back in the left column.
    assert_quads(&quads_of(cx, l), &[(0.0, y1, 499.0, 40.0)]);
    assert_quads(&quads_of(cx, r), &[(500.0, y2, 500.0, 50.0)]);
}

#[gpui_kit::test]
fn block_height_change_keeps_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // 60 context-free added lines: one row per line.
    let text = numbered("line", 60).concat();
    let provider = MemProvider::new(vec![Spec::added("src/a.rs", &text)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let (spec, height) = resizable(1, new(4), 60.0, color(0.3));
    set_blocks(&view, cx, 0, vec![spec]);
    let line = |n: u32| unified(None, Some(n + 1), '+', &format!("line {n}"));

    // Visible: the rows above it and the anchor stay, the rows below move.
    let before = debug(&view, cx);
    height.set(100.0);
    view.update(cx, |v, cx| v.invalidate_block(BlockId(1), cx));
    settle(cx);
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(
        bounds_of(&view, cx, &line(3)),
        (HEADER_H + 3.0 * ROW_H, ROW_H)
    );
    assert_eq!(bounds_of(&view, cx, "[block 1]").1, 100.0);
    assert_eq!(
        bounds_of(&view, cx, &line(5)).0,
        HEADER_H + 5.0 * ROW_H + 100.0
    );

    // Above the viewport (but near it): re-measured as soon as it is
    // invalidated, and nothing on screen moves.
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 20,
            },
            cx,
        )
    });
    settle(cx);
    let before = debug(&view, cx);
    // The line lands right below the file's pinned header (T2.5), which
    // covers the two rows above it.
    assert_eq!(before.visible_rows[0], "== src/a.rs");
    assert_eq!(bounds_of(&view, cx, &line(20)), (HEADER_H, ROW_H));
    height.set(260.0);
    view.update(cx, |v, cx| v.invalidate_block(BlockId(1), cx));
    settle(cx);
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows, before.visible_rows);
    assert_eq!(after.row_bounds, before.row_bounds);
    view.read_with(cx, |v, _| {
        assert_eq!(v.document().blocks(0)[0].height, 260.0)
    });

    // Scrolling up into it moves everything by exactly the wheel delta: its
    // height was already right.
    let y = bounds_of(&view, cx, &line(20)).0;
    wheel(cx, -350.0);
    assert_eq!(
        debug(&view, cx).visible_rows[..2],
        ["== src/a.rs", "[block 1]"]
    );
    // Its bottom is where line 5 starts, 15 rows above line 20.
    assert_eq!(
        bounds_of(&view, cx, "[block 1]"),
        (y + 350.0 - 15.0 * ROW_H - 260.0, 260.0)
    );
    assert_eq!(bounds_of(&view, cx, &line(20)).0, y + 350.0);
}

#[gpui_kit::test]
fn block_scrolled_into_from_below_keeps_rows_below_still(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let text = numbered("line", 300).concat();
    let provider = MemProvider::new(vec![Spec::added("src/a.rs", &text)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 200,
            },
            cx,
        )
    });
    settle(cx);
    // Far above the viewport (beyond the measuring band): only estimated.
    set_blocks(&view, cx, 0, vec![boxed(1, new(10), 200.0, color(0.5))]);
    let estimate = view.read_with(cx, |v, _| v.document().blocks(0)[0].height);
    assert_eq!(estimate, ESTIMATED_BLOCK_ROWS * ROW_H);

    // One wheel step up to 20 px inside the block.
    let (block_top, scroll_top) = view.read_with(cx, |v, _| {
        let d = v.document();
        let layout = d.file_layout(0).unwrap();
        let row = layout.find(RowKey::Block(BlockId(1))).unwrap();
        (
            d.file_top(0) + f64::from(HEADER_H) + layout.row_top(row),
            d.scroll_top(),
        )
    });
    wheel(cx, (block_top + 20.0 - scroll_top) as f32);

    // Measured at 200: the line below it stays where the wheel put it (the
    // block's bottom edge is fixed), so the block grows upwards.
    let below = unified(None, Some(12), '+', "line 11");
    assert_eq!(bounds_of(&view, cx, &below).0, estimate - 20.0);
    assert_eq!(
        bounds_of(&view, cx, "[block 1]"),
        (estimate - 20.0 - 200.0, 200.0)
    );
}

#[gpui_kit::test]
fn set_blocks_relayouts_only_that_file(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let mut specs: Vec<Spec> = (0..3)
        .map(|i| Spec::modified(&format!("src/f{i}.rs"), "a\nb\nc\n", "a\nB\nc\n"))
        .collect();
    // Far below, never laid out.
    specs
        .extend((3..40).map(|i| Spec::added(&format!("src/g{i}.rs"), &numbered("x", 50).concat())));
    let provider = MemProvider::new(specs);
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    let snapshot = |cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            let d = v.document();
            (0..d.len())
                .map(|f| (d.file_layout(f).cloned(), d.generation(f)))
                .collect::<Vec<_>>()
        })
    };
    let before = snapshot(cx);
    let loads = provider.load_count();
    assert!(before[39].0.is_none());

    set_blocks(&view, cx, 1, vec![boxed(1, new(1), 40.0, color(0.1))]);
    let after = snapshot(cx);
    for f in (0..40).filter(|&f| f != 1) {
        assert_eq!(after[f], before[f], "file {f} changed");
    }
    // File 1: the same rows plus the block, not reloaded.
    assert_eq!(after[1].1, before[1].1);
    assert_eq!(provider.load_count(), loads);
    assert_eq!(
        describe(after[1].0.as_ref().unwrap()),
        strs(&["o0 n0", "o1 n-", "o- n1", "[1]", "o2 n2"])
    );

    // An id reused in another file moves the block there.
    set_blocks(&view, cx, 2, vec![boxed(1, new(0), 40.0, color(0.1))]);
    assert_eq!(
        layout_rows(&view, cx, 1),
        strs(&["o0 n0", "o1 n-", "o- n1", "o2 n2"])
    );
    assert_eq!(layout_rows(&view, cx, 2)[1], "[1]");
    view.read_with(cx, |v, _| {
        assert!(v.document().blocks(1).is_empty());
        assert_eq!(v.document().blocks(2).len(), 1);
    });
    // Blocks of a file that is not laid out wait for its layout.
    set_blocks(
        &view,
        cx,
        39,
        vec![boxed(2, BlockAnchor::FileTop, 40.0, color(0.1))],
    );
    view.read_with(cx, |v, _| {
        assert!(v.document().file_layout(39).is_none());
        assert_eq!(v.document().blocks(39).len(), 1);
    });
}

#[gpui_kit::test]
fn scroll_to_block_lands_on_it_before_its_file_is_laid_out(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let specs = (0..10)
        .map(|i| Spec::added(&format!("src/f{i}.rs"), &numbered("line", 60).concat()))
        .collect();
    let (view, cx) = open(
        cx,
        MemProvider::new(specs),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    set_blocks(&view, cx, 8, vec![boxed(5, new(20), 90.0, color(0.9))]);
    view.read_with(cx, |v, _| assert!(v.document().file_layout(8).is_none()));

    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::Block(BlockId(5)), cx));
    settle(cx);
    let d = debug(&view, cx);
    // Right below the file's pinned header, like a line (T2.5).
    assert_eq!(d.visible_rows[0], "== src/f8.rs");
    assert_eq!(bounds_of(&view, cx, "[block 5]"), (HEADER_H, 90.0));
    assert_eq!(
        bounds_of(&view, cx, &unified(None, Some(22), '+', "line 21")),
        (HEADER_H + 90.0, ROW_H)
    );
    assert_eq!(d.anchor.file_idx, 8);
    assert_eq!(d.anchor.row, RowKey::Block(BlockId(5)));
    assert_eq!(d.anchor.offset_px, -HEADER_H);

    // Unknown blocks are ignored.
    view.update(cx, |v, cx| {
        v.scroll_to(ScrollTarget::Block(BlockId(99)), cx)
    });
    settle(cx);
    assert_eq!(debug(&view, cx).anchor, d.anchor);
}

#[gpui_kit::test]
fn block_elements_are_interactive_and_the_wheel_still_scrolls(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let text = numbered("line", 80).concat();
    let provider = MemProvider::new(vec![Spec::added("src/a.rs", &text)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1000., 400.);
    let clicks = Rc::new(Cell::new(0));
    let counter = clicks.clone();
    let spec = BlockSpec {
        id: BlockId(1),
        anchor: new(2),
        render: Rc::new(move |_, _| {
            let counter = counter.clone();
            div()
                .id("button")
                .w_full()
                .h(px(80.))
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    counter.set(counter.get() + 1)
                })
                .into_any_element()
        }),
    };
    set_blocks(&view, cx, 0, vec![spec]);
    let (y, h) = bounds_of(&view, cx, "[block 1]");
    assert_eq!(h, 80.0);

    // The block sits in the right column: a click there reaches it, a click
    // on the spacer does not.
    cx.simulate_click(point(px(700.), px(y + 40.)), Modifiers::default());
    cx.simulate_click(point(px(200.), px(y + 40.)), Modifiers::default());
    assert_eq!(clicks.get(), 1);

    let anchor = debug(&view, cx).anchor;
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: point(px(700.), px(y + 40.)),
        delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-60.))),
        modifiers: Modifiers::default(),
        ..Default::default()
    });
    settle(cx);
    assert_ne!(debug(&view, cx).anchor, anchor);
    assert_eq!(bounds_of(&view, cx, "[block 1]").0, y - 60.0);
}

#[gpui_kit::test]
fn pinned_header_covers_blocks_and_takes_their_clicks(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let text = numbered("line", 80).concat();
    let provider = MemProvider::new(vec![Spec::added("src/a.rs", &text)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let clicks = Rc::new(Cell::new(0));
    let counter = clicks.clone();
    let spec = BlockSpec {
        id: BlockId(1),
        anchor: new(2),
        render: Rc::new(move |_, _| {
            let counter = counter.clone();
            div()
                .id("card")
                .w_full()
                .h(px(200.))
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    counter.set(counter.get() + 1)
                })
                .into_any_element()
        }),
    };
    set_blocks(&view, cx, 0, vec![spec]);
    assert_eq!(
        bounds_of(&view, cx, "[block 1]"),
        (HEADER_H + 3.0 * ROW_H, 200.0)
    );

    // Scrolled so that the block's top runs under the file's pinned header
    // (T2.5): the header is painted over it and takes the clicks there.
    wheel(cx, 160.0);
    let d = debug(&view, cx);
    assert_eq!(bounds_of(&view, cx, "[block 1]"), (-60.0, 200.0));
    let h = &d.headers[0];
    assert!(h.sticky && h.y == 0.0, "{h:?}");
    cx.simulate_click(point(px(500.), px(HEADER_H / 2.0)), Modifiers::default());
    assert_eq!(clicks.get(), 0);
    // Below the header, the block still gets them.
    cx.simulate_click(point(px(500.), px(HEADER_H + 40.)), Modifiers::default());
    assert_eq!(clicks.get(), 1);
}

#[gpui_kit::test]
fn new_block_is_measured_in_the_first_frame_and_rendered_once(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let (events, _sub) = record_events(&view, cx);
    let renders = Rc::new(Cell::new(0));
    let count = renders.clone();
    let spec = BlockSpec {
        id: BlockId(1),
        anchor: new(4),
        render: Rc::new(move |_, _| {
            count.set(count.get() + 1);
            div().w_full().h(px(120.)).into_any_element()
        }),
    };
    let frames = all_stats(&events).len();

    // The next frame (plan T2.9 `comment_repaint_ms`: set_blocks → next
    // completed frame; the update draws it) already shows the block at its
    // measured height: the frame was rebuilt around it, without rendering it
    // twice.
    view.update(cx, |v, cx| v.set_blocks(0, vec![spec], cx));
    assert_eq!(all_stats(&events).len(), frames + 1);
    let top = HEADER_H + 32.0 + 5.0 * ROW_H;
    assert_eq!(bounds_of(&view, cx, "[block 1]"), (top, 120.0));
    assert_eq!(
        bounds_of(&view, cx, &unified(Some(6), Some(6), ' ', "line 5")).0,
        top + 120.0
    );
    assert_eq!(renders.get(), 1);
    // Every later frame renders it once more (elements live for one frame).
    redraw(cx);
    assert_eq!(renders.get(), 2);
}
