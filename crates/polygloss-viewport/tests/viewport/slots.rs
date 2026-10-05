//! Display order and hidden files (T6.10, design §11.6 "Sections", §11.15):
//! the height index is by display slot, every API stays keyed by `file_idx`,
//! hidden files have no height and are never walked, and the anchor stays
//! put (or at the top) when the order or the hidden set changes.

use std::sync::Arc;

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use polygloss_diff::{
    FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, Mode, ObjectFormat, Oid, Side,
};
use polygloss_viewport::document::{
    BlockAnchor, BlockId, BodyRow, Document, FileLayout, FileState, HeightIndex, Metrics,
    PlacedBlock, RowKey, ScrollAnchor, SlotRange,
};
use polygloss_viewport::{CursorPos, DiffViewport, LayoutMode};

use crate::support::*;

// ---------------------------------------------------------------------------
// helpers

fn modified(idx: u32) -> FileChange {
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
        generated_attr: GeneratedAttr::Unspecified,
    }
}

fn slots(start: u32, end: u32) -> SlotRange {
    SlotRange { start, end }
}

fn anchor(file_idx: u32, row: RowKey, offset_px: f32) -> ScrollAnchor {
    ScrollAnchor {
        file_idx,
        row,
        offset_px,
    }
}

/// Flat v1 geometry: 40 px headers, no card gap, no padding.
fn flat() -> Metrics {
    Metrics {
        header_height: 40.0,
        ..Metrics::default()
    }
}

/// Cards: 40 px headers, 12 px between cards (and below the last one), 8 px
/// of padding under a non-empty body.
fn cards() -> Metrics {
    Metrics {
        header_height: 40.0,
        card_gap: 12.0,
        card_pad_bottom: 8.0,
        ..Metrics::default()
    }
}

/// A document of `heights.len()` files whose header and body are exactly
/// `heights[f]` px tall (the lead and the card's padding come on top).
fn doc_with(metrics: Metrics, heights: &[f32], viewport_h: f32) -> Document {
    let files = Arc::new((0..heights.len() as u32).map(modified).collect());
    let mut d = Document::new(files, metrics);
    d.set_viewport_height(viewport_h);
    for (f, &h) in heights.iter().enumerate() {
        d.set_file_height(f as u32, h);
    }
    d
}

fn shown(d: &Document, range: SlotRange) -> Vec<u32> {
    d.shown_files(range).collect()
}

fn tops(d: &Document) -> Vec<f64> {
    (0..d.len()).map(|f| d.file_top(f)).collect()
}

/// `n` added text files `f<i>.txt` of three lines each.
fn three_line_files(n: u32) -> Vec<Spec> {
    (0..n)
        .map(|i| {
            Spec::added(
                &format!("f{i}.txt"),
                &numbered(&format!("f{i}"), 3).concat(),
            )
        })
        .collect()
}

fn cursor_pos(file_idx: u32, line: u32) -> CursorPos {
    CursorPos {
        file_idx,
        side: Side::New,
        line,
        range_start: None,
    }
}

// ---------------------------------------------------------------------------
// the document

#[test]
fn identity_order_keeps_every_v1_position() {
    // Flat: each file is its header and body, one after the other.
    let flat = doc_with(flat(), &[100.0, 200.0, 50.0, 80.0, 120.0], 200.0);
    assert_eq!(flat.display_order(), &[0, 1, 2, 3, 4]);
    assert_eq!(tops(&flat), [0.0, 100.0, 300.0, 350.0, 430.0]);
    assert_eq!(flat.total_height(), 550.0);

    // Cards with a 30 px prelude: file 0's lead is the prelude and a gap,
    // every other lead a gap, and the gap below the last card is file 4's.
    // Heights: 42+40+68, 12+40+168, 12+40+18, 12+40+48, 12+40+88+12.
    let mut d = doc_with(cards(), &[100.0, 200.0, 50.0, 80.0, 120.0], 200.0);
    d.set_prelude_height(Some(30.0));
    for f in 0..5 {
        assert_eq!(d.slot(f), f);
        assert_eq!(d.file_at(f), f);
        assert!(!d.is_hidden(f));
    }
    assert_eq!(d.top_anchor(), ScrollAnchor::default());
    assert_eq!(*d.anchor(), ScrollAnchor::default());
    assert_eq!(tops(&d), [0.0, 150.0, 370.0, 440.0, 540.0]);
    let header_tops: Vec<f64> = (0..5).map(|f| d.header_top(f)).collect();
    assert_eq!(header_tops, [42.0, 162.0, 382.0, 452.0, 552.0]);
    let card_bottoms: Vec<f64> = (0..5).map(|f| d.card_bottom(f)).collect();
    assert_eq!(card_bottoms, [150.0, 370.0, 440.0, 540.0, 680.0]);
    assert_eq!(d.total_height(), 692.0);
    assert_eq!(d.visible(200.0), slots(0, 2));
    assert_eq!(shown(&d, slots(0, 5)), [0, 1, 2, 3, 4]);

    d.scroll_to(2, RowKey::Header);
    assert_eq!(d.scroll_top(), 382.0);
    // [382, 582): files 2 (370..440), 3 (440..540) and 4 (540..692).
    assert_eq!(d.visible(200.0), slots(2, 5));
    // One screen around it: [182, 782), cut at the document's end.
    assert_eq!(d.materialize_range(200.0, 1.0), slots(1, 5));
    assert_eq!(d.file_at_offset(450.0), (3, 10.0));

    // Setting the identity order again changes nothing.
    let (before, scroll, at) = (tops(&d), d.scroll_top(), *d.anchor());
    d.set_order(vec![0, 1, 2, 3, 4]);
    assert_eq!(
        (tops(&d), d.scroll_top(), *d.anchor()),
        (before, scroll, at)
    );
}

#[test]
fn permuted_order_lays_files_out_in_display_order() {
    let mut d = doc_with(cards(), &[100.0, 200.0, 50.0, 80.0], 150.0);
    d.set_prelude_height(Some(30.0));
    d.set_order(vec![2, 0, 3, 1]);
    assert_eq!(d.display_order(), &[2, 0, 3, 1]);
    let slot_of: Vec<u32> = (0..4).map(|f| d.slot(f)).collect();
    assert_eq!(slot_of, [1, 3, 0, 2]);
    let file_of: Vec<u32> = (0..4).map(|s| d.file_at(s)).collect();
    assert_eq!(file_of, [2, 0, 3, 1]);
    // File 2 comes first with the prelude in its lead (42+40+18); then 0
    // (12+40+68), 3 (12+40+48) and 1, which holds the gap below the last
    // card (12+40+168+12).
    assert_eq!(tops(&d), [100.0, 320.0, 0.0, 220.0]);
    let heights: Vec<f32> = (0..4).map(|f| d.file_height(f)).collect();
    assert_eq!(heights, [120.0, 232.0, 100.0, 100.0]);
    let header_tops: Vec<f64> = (0..4).map(|f| d.header_top(f)).collect();
    assert_eq!(header_tops, [112.0, 332.0, 42.0, 232.0]);
    assert_eq!((d.lead(2), d.lead(0)), (42.0, 12.0));
    assert_eq!(d.card_bottom(1), 540.0);
    assert_eq!(d.total_height(), 552.0);
    assert_eq!(d.file_at_offset(105.0), (0, 5.0));
    assert_eq!(d.file_at_offset(320.0), (1, 0.0));

    // The top is the first slot's file.
    assert_eq!(d.top_anchor(), anchor(2, RowKey::Lead, 0.0));
    assert_eq!(*d.anchor(), anchor(2, RowKey::Lead, 0.0));
    assert_eq!(d.scroll_top(), 0.0);
    // [0, 150): file 2 (0..100) and file 0 (100..220).
    assert_eq!(d.visible(150.0), slots(0, 2));
    assert_eq!(shown(&d, d.visible(150.0)), [2, 0]);
    assert_eq!(shown(&d, slots(0, 4)), [2, 0, 3, 1]);
}

#[test]
fn apis_stay_keyed_by_file_idx_under_a_permutation() {
    let mut d = doc_with(cards(), &[100.0, 200.0, 50.0, 80.0], 100.0);
    d.set_prelude_height(Some(30.0));
    d.set_order(vec![2, 0, 3, 1]);

    // Collapsing file 0 (slot 1) leaves its lead and header: 12 + 40.
    d.set_collapsed(0, true);
    assert!(d.is_collapsed(0) && !d.is_collapsed(2));
    assert_eq!(d.file_height(0), 52.0);
    assert_eq!(d.file_height(2), 100.0);
    assert_eq!(d.file_top(3), 152.0);

    // File 3's exact height (slot 2): body 100, so 12 + 40 + 108.
    d.set_file_height(3, 140.0);
    assert_eq!(d.file_height(3), 160.0);
    assert_eq!(d.file_top(1), 312.0);
    assert_eq!(d.key_offset(1, RowKey::Header), Some(12.0));

    // A block in file 1 adds to its explicit body: 12 + 40 + 190 + 8 + 12.
    d.set_blocks(
        1,
        vec![PlacedBlock {
            id: BlockId(9),
            anchor: BlockAnchor::FileTop,
            height: 30.0,
        }],
    );
    assert_eq!(d.file_height(1), 262.0);
    assert_eq!(d.blocks(1).len(), 1);
    assert!(d.blocks(3).is_empty());

    // Loading states are per file.
    let generation = d.begin_loading(3);
    assert!(matches!(d.state(3), FileState::Loading { generation: g } if *g == generation));
    assert!(matches!(d.state(2), FileState::Estimated));

    // Scrolling to a file brings that file's header up.
    d.scroll_to(3, RowKey::Header);
    assert_eq!(*d.anchor(), anchor(3, RowKey::Header, 0.0));
    assert_eq!(d.scroll_top(), 164.0);
    assert_eq!(d.file_at_offset(d.scroll_top()), (3, 12.0));
}

#[test]
fn shown_files_skips_hidden_slots_by_offset() {
    // 10,005 files of 440 px (estimated); 10,000 of them hidden.
    let n = 10_005;
    let files = Arc::new((0..n).map(modified).collect());
    let mut d = Document::new(files, flat());
    d.set_viewport_height(1_000.0);
    let hidden: Vec<u32> = (1..=10_000).collect();
    d.set_hidden(&hidden, true);
    assert!(d.is_hidden(1) && d.is_hidden(10_000) && !d.is_hidden(10_001));
    assert_eq!(d.file_height(5_000), 0.0);
    assert_eq!(d.total_height(), 5.0 * 440.0);

    // The whole document: one look per shown file; the walk ends at the
    // document's end without looking at another slot.
    let mut all = d.shown_files(slots(0, n));
    assert_eq!(
        all.by_ref().collect::<Vec<_>>(),
        [0, 10_001, 10_002, 10_003, 10_004]
    );
    assert_eq!(all.visits(), 5);
    // A range that ends before the last file: one more look, at the slot
    // past it.
    let mut most = d.shown_files(slots(0, n - 1));
    assert_eq!(
        most.by_ref().collect::<Vec<_>>(),
        [0, 10_001, 10_002, 10_003]
    );
    assert_eq!(most.visits(), 4 + 1);
    // A range starting inside the hidden run.
    let mut inner = d.shown_files(slots(5, 10_003));
    assert_eq!(inner.by_ref().collect::<Vec<_>>(), [10_001, 10_002]);
    assert_eq!(inner.visits(), 2 + 1);
    assert_eq!(shown(&d, slots(3, 3)), Vec::<u32>::new());

    // [0, 1000): file 0 (0..440), 10,001 (440..880) and 10,002 (880..1320).
    let visible = d.visible(1_000.0);
    assert_eq!(visible, slots(0, 10_003));
    assert_eq!(shown(&d, visible), [0, 10_001, 10_002]);
    assert!(d.contains_file(visible, 10_001));
    assert!(
        !d.contains_file(visible, 7),
        "hidden files are never in a range"
    );
    assert!(!d.contains_file(visible, 10_003));
}

#[test]
fn overlapping_clamps_at_the_document_end() {
    // Six 100 px files; the last two in display order are hidden.
    let mut d = doc_with(flat(), &[100.0; 6], 150.0);
    d.set_order(vec![0, 1, 2, 3, 5, 4]);
    d.set_hidden(&[4, 5], true);
    assert_eq!(d.total_height(), 400.0);
    d.scroll_by(10_000.0);
    assert_eq!(d.scroll_top(), 250.0);
    // [250, 400) reaches the document's end: the range ends at the last
    // shown slot, not at the hidden ones after it.
    assert_eq!(d.visible(150.0), slots(2, 4));
    assert_eq!(d.visible(1_000.0), slots(2, 4));
    assert_eq!(d.materialize_range(150.0, 2.0), slots(0, 4));
    assert_eq!(shown(&d, d.visible(150.0)), [2, 3]);
    // Past the end, the file at an offset is the last shown one too.
    assert_eq!(d.file_at_offset(400.0), (3, 100.0));
    assert_eq!(d.file_at_offset(450.0), (3, 150.0));
}

#[test]
fn set_order_keeps_the_anchor() {
    let mut d = doc_with(flat(), &[100.0, 200.0, 50.0, 80.0], 100.0);
    // 30 px into file 1's header.
    d.scroll_to(1, RowKey::Header);
    d.scroll_by(30.0);
    assert_eq!(*d.anchor(), anchor(1, RowKey::Header, 30.0));
    assert_eq!(d.scroll_top(), 130.0);

    // File 1 moves to the end: 50 + 100 + 80 above it now.
    d.set_order(vec![2, 0, 3, 1]);
    assert_eq!(*d.anchor(), anchor(1, RowKey::Header, 30.0));
    assert_eq!(d.scroll_top(), 260.0);

    // A line anchor in a laid-out file stays on its line.
    let rows: Vec<BodyRow> = (0..20)
        .map(|i| BodyRow::Line {
            old: Some(i),
            new: Some(i),
            diff_row: i,
        })
        .collect();
    d.set_file_layout(3, FileLayout::new(rows, &[20.0; 20]));
    d.scroll_to(
        3,
        RowKey::Line {
            side: Side::New,
            line: 4,
        },
    );
    d.scroll_by(5.0);
    let at = *d.anchor();
    assert_eq!(
        at,
        anchor(
            3,
            RowKey::Line {
                side: Side::New,
                line: 4
            },
            5.0
        )
    );
    // Order 3, 2, 0, 1: file 3 is first, line 4 is 40 + 4 × 20 into it.
    d.set_order(vec![3, 2, 0, 1]);
    assert_eq!(*d.anchor(), at);
    assert_eq!(d.scroll_top(), 125.0);
}

#[test]
fn top_anchor_stays_at_the_top_across_set_order_and_set_hidden() {
    let mut d = doc_with(cards(), &[100.0, 200.0, 50.0, 80.0], 100.0);
    d.set_prelude_height(Some(30.0));
    assert_eq!(*d.anchor(), d.top_anchor());

    d.set_order(vec![2, 0, 3, 1]);
    assert_eq!(*d.anchor(), anchor(2, RowKey::Lead, 0.0));
    assert_eq!(d.scroll_top(), 0.0);
    assert_eq!(
        d.header_top(2),
        42.0,
        "the prelude is in the first card's lead"
    );

    // Hiding the first shown file: the next one is at the top, with the
    // prelude.
    d.set_hidden(&[2], true);
    assert_eq!(d.top_anchor(), anchor(0, RowKey::Lead, 0.0));
    assert_eq!(*d.anchor(), anchor(0, RowKey::Lead, 0.0));
    assert_eq!(d.scroll_top(), 0.0);
    assert_eq!(d.file_height(2), 0.0);
    assert_eq!(d.header_top(0), 42.0);

    // Showing it again: back at the top.
    d.set_hidden(&[2], false);
    assert_eq!(*d.anchor(), anchor(2, RowKey::Lead, 0.0));
    assert_eq!(d.header_top(2), 42.0);
    assert_eq!(d.header_top(0), 112.0);

    // An anchor inside the prelude follows it to the new first file.
    d.scroll_by(20.0);
    assert_eq!(*d.anchor(), anchor(2, RowKey::Lead, 20.0));
    d.set_order(vec![3, 2, 0, 1]);
    assert_eq!(*d.anchor(), anchor(3, RowKey::Lead, 20.0));
    assert_eq!(d.scroll_top(), 20.0);
    d.set_hidden(&[3], true);
    assert_eq!(*d.anchor(), anchor(2, RowKey::Lead, 20.0));
    assert_eq!(d.scroll_top(), 20.0);
    d.set_hidden(&[3], false);
    d.set_order(vec![2, 0, 3, 1]);
    assert_eq!(*d.anchor(), anchor(2, RowKey::Lead, 20.0));

    // An anchor below the top is not moved to it.
    d.scroll_to(3, RowKey::Header);
    assert_eq!(d.scroll_top(), 232.0);
    d.set_hidden(&[2], true);
    assert_eq!(*d.anchor(), anchor(3, RowKey::Header, 0.0));
    // File 0 (42+40+68 with the prelude) is above it now.
    assert_eq!(d.scroll_top(), 162.0);
}

#[test]
fn height_index_finds_the_last_item_before_an_offset() {
    let idx = HeightIndex::new(&[10.0, 0.0, 30.0, 0.0, 5.0]);
    // The item holding the pixel just above the offset: empty items are
    // skipped.
    assert_eq!(idx.last_before(0.0), None);
    assert_eq!(idx.last_before(10.0), Some(0));
    assert_eq!(idx.last_before(10.5), Some(2));
    assert_eq!(idx.last_before(40.0), Some(2));
    assert_eq!(idx.last_before(45.0), Some(4));
    assert_eq!(idx.last_before(100.0), Some(4));
    assert_eq!(HeightIndex::new(&[0.0, 0.0]).last_before(5.0), None);
}

// ---------------------------------------------------------------------------
// the view

fn cursor(view: &Entity<DiffViewport>, cx: &mut VisualTestContext) -> Option<CursorPos> {
    view.read_with(cx, |v, _| v.cursor())
}

#[gpui_kit::test]
fn hiding_the_anchor_file_moves_the_anchor_and_drops_the_cursor(cx: &mut TestAppContext) {
    // Pure: 30 px into file 1, which is hidden; the anchor is the top of
    // where it was.
    let mut d = doc_with(flat(), &[100.0, 200.0, 50.0, 80.0], 100.0);
    d.scroll_to(1, RowKey::Header);
    d.scroll_by(30.0);
    d.set_hidden(&[1], true);
    assert_eq!(*d.anchor(), anchor(1, RowKey::Lead, 0.0));
    assert_eq!(d.scroll_top(), 100.0);
    assert_eq!(shown(&d, d.visible(100.0)), [2, 3]);

    // The view: three files of 30 added lines (45 + 600 px each).
    let _sb = sandbox();
    let specs = (0..3)
        .map(|i| Spec::added(&format!("f{i}.txt"), &numbered("line", 30).concat()))
        .collect();
    let (view, cx) = open(
        cx,
        MemProvider::new(specs),
        options(LayoutMode::Unified),
        1000.,
        300.,
    );
    view.update(cx, |v, cx| v.set_cursor(Some(cursor_pos(1, 5)), cx));
    settle(cx);
    assert_eq!(view.read_with(cx, |v, _| v.anchor().file_idx), 1);
    // Hiding another file keeps the cursor.
    view.update(cx, |v, cx| v.set_hidden(&[2], true, cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), Some(cursor_pos(1, 5)));
    view.update(cx, |v, cx| v.set_hidden(&[2], false, cx));
    settle(cx);

    view.update(cx, |v, cx| v.set_hidden(&[1], true, cx));
    settle(cx);
    assert_eq!(cursor(&view, cx), None);
    assert!(view.read_with(cx, |v, _| v.is_hidden(1)));
    assert_eq!(
        view.read_with(cx, |v, _| v.anchor()),
        anchor(1, RowKey::Lead, 0.0)
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.document().scroll_top()),
        f64::from(HEADER_H + 30.0 * ROW_H)
    );
    // What shows there is file 2, from its header.
    let d = debug(&view, cx);
    assert_eq!((d.headers[0].file_idx, d.headers[0].y), (2, 0.0));
    assert!(d.headers.iter().all(|h| h.file_idx != 1));
}

#[gpui_kit::test]
fn display_rank_and_order_follow_set_order(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        MemProvider::new(three_line_files(4)),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    view.update(cx, |v, cx| v.set_order(vec![3, 1, 0, 2], cx));
    settle(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.display_order(), &[3, 1, 0, 2]);
        let ranks: Vec<u32> = (0..4).map(|f| v.display_rank(f)).collect();
        assert_eq!(ranks, [2, 1, 3, 0]);
    });
    // Headers are painted in display order.
    let d = debug(&view, cx);
    let painted: Vec<u32> = d.headers.iter().map(|h| h.file_idx).collect();
    assert_eq!(painted, [3, 1, 0, 2]);
    // Each card is its header and three rows: 45 + 60 px.
    let ys: Vec<f32> = d.headers.iter().map(|h| h.y).collect();
    assert_eq!(ys, [0.0, 105.0, 210.0, 315.0]);
}
