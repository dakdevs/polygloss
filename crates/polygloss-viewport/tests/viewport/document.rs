//! Viewport document model (T2.3, design §12.4): height index, logical scroll
//! anchor, estimates, materialization window and eviction.

use std::sync::Arc;
use std::time::{Duration, Instant};

use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::{Expansions, GapId, Layout, Row, build_rows};
use polygloss_diff::{
    FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, Mode, ObjectFormat, Oid, Side,
};
use polygloss_viewport::document::{
    BlockAnchor, BlockId, BodyRow, DEFAULT_EVICTION_BUDGET_BYTES, DEFAULT_WINDOW_SCREENS, Document,
    FileLayout, FileState, HeightIndex, Metrics, PlacedBlock, RowKey, ScrollAnchor, SizeHint,
    SlotRange,
};
use polygloss_viewport::materialize::MaterializedFile;

// ---------------------------------------------------------------------------
// helpers

const BLOB_A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
const BLOB_B: &str = "5716ca5987cbf97d6bb54920bea6adde242d87e6";

fn oid(s: &str) -> Oid {
    Oid::parse(s, ObjectFormat::Sha1).unwrap()
}

fn path(p: &str) -> Option<GitPath> {
    Some(GitPath::from_bytes(p.as_bytes()))
}

/// A modified text file `src/file-<idx>.rs`.
fn modified(idx: u32) -> FileChange {
    let p = format!("src/file-{idx}.rs");
    FileChange {
        idx,
        status: FileStatus::Modified,
        old_path: path(&p),
        new_path: path(&p),
        old_mode: Some(Mode(0o100644)),
        new_mode: Some(Mode(0o100644)),
        old_blob: oid(BLOB_A),
        new_blob: oid(BLOB_B),
        similarity: None,
        kind: FileKind::Text,
        generated: false,
        generated_attr: GeneratedAttr::Unspecified,
    }
}

fn slots(start: u32, end: u32) -> SlotRange {
    SlotRange { start, end }
}

fn files(n: u32) -> Arc<Vec<FileChange>> {
    Arc::new((0..n).map(modified).collect())
}

/// The document model's geometry in these tests: the default metrics with a
/// 40 px header, which the hand-computed positions below are written for
/// (the default header's own height is pinned by `cards.rs`).
fn metrics() -> Metrics {
    Metrics {
        header_height: 40.0,
        ..Metrics::default()
    }
}

fn doc(n: u32) -> Document {
    Document::new(files(n), metrics())
}

/// A document of `heights.len()` files with exact heights (header included).
fn doc_with_heights(heights: &[f32], viewport_h: f32) -> Document {
    let mut d = doc(heights.len() as u32);
    d.set_viewport_height(viewport_h);
    for (i, &h) in heights.iter().enumerate() {
        d.set_file_height(i as u32, h);
    }
    d
}

/// A body of `n` unified context rows (old = new = line), each `row_h` tall.
fn context_layout(n: u32, row_h: f32) -> FileLayout {
    let rows = (0..n)
        .map(|i| BodyRow::Line {
            old: Some(i),
            new: Some(i),
            diff_row: i,
        })
        .collect();
    FileLayout::new(rows, &vec![row_h; n as usize])
}

/// Where row `row` of file `idx`'s body is on screen (0 = viewport top).
fn screen_y(d: &Document, idx: u32, row: usize) -> f64 {
    let layout = d.file_layout(idx).expect("file is laid out");
    d.file_top(idx) + f64::from(d.metrics().header_height) + layout.row_top(row) - d.scroll_top()
}

/// Screen positions of every row of the laid-out files in `files`.
fn snapshot(d: &Document, files: &[u32]) -> Vec<(u32, usize, f64)> {
    let mut out = Vec::new();
    for &f in files {
        for r in 0..d.file_layout(f).unwrap().len() {
            out.push((f, r, screen_y(d, f, r)));
        }
    }
    out
}

fn numbered(n: u32) -> Vec<String> {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

/// 100 numbered lines; new edits lines 10, 50 and 90. Hunks around each edit,
/// gaps between them.
fn three_edits() -> (Vec<u8>, Vec<u8>) {
    let old = numbered(100);
    let mut new = old.clone();
    for i in [10, 50, 90] {
        new[i] = format!("LINE {i}\n");
    }
    (old.concat().into_bytes(), new.concat().into_bytes())
}

fn rows_of(old: &[u8], new: &[u8], exp: &Expansions, layout: Layout) -> Vec<Row> {
    build_rows(&diff_blobs(old, new, &DiffOptions::default()), exp, layout)
}

/// Index of the first body row showing `line` of `side`.
fn row_showing(layout: &FileLayout, side: Side, line: u32) -> usize {
    layout
        .rows()
        .iter()
        .position(|r| match (r, side) {
            (BodyRow::Line { old, .. }, Side::Old) => *old == Some(line),
            (BodyRow::Line { new, .. }, Side::New) => *new == Some(line),
            _ => false,
        })
        .unwrap_or_else(|| panic!("no row shows {side:?} {line}"))
}

fn materialized(heap_bytes: usize) -> Arc<MaterializedFile> {
    let diff = diff_blobs(b"a\n", b"b\n", &DiffOptions::default());
    let mut f = MaterializedFile::new(diff, vec![None]);
    f.heap_bytes = heap_bytes;
    Arc::new(f)
}

/// A small deterministic PRNG (xorshift64*).
fn next(x: &mut u64) -> u64 {
    *x ^= *x >> 12;
    *x ^= *x << 25;
    *x ^= *x >> 27;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

// ---------------------------------------------------------------------------
// HeightIndex

#[test]
fn height_index_prefix_and_find() {
    let mut idx = HeightIndex::new(&[10.0, 0.0, 30.0, 5.0]);
    assert_eq!(idx.len(), 4);
    assert_eq!(idx.total(), 45.0);
    let prefixes: Vec<f64> = (0..=5).map(|i| idx.prefix(i)).collect();
    assert_eq!(prefixes, [0.0, 10.0, 10.0, 40.0, 45.0, 45.0]);

    assert_eq!(idx.find(0.0), (0, 0.0));
    assert_eq!(idx.find(9.5), (0, 9.5));
    // A boundary belongs to the item starting there; zero-height items are skipped.
    assert_eq!(idx.find(10.0), (2, 0.0));
    assert_eq!(idx.find(39.0), (2, 29.0));
    assert_eq!(idx.find(40.0), (3, 0.0));
    // Past the end: the last item. Below 0: clamped.
    assert_eq!(idx.find(45.0), (3, 5.0));
    assert_eq!(idx.find(100.0), (3, 60.0));
    assert_eq!(idx.find(-5.0), (0, 0.0));

    idx.set(1, 7.0);
    assert_eq!(idx.get(1), 7.0);
    assert_eq!(idx.prefix(2), 17.0);
    assert_eq!(idx.total(), 52.0);
    assert_eq!(idx.find(12.0), (1, 2.0));

    // Invalid heights count as 0.
    idx.set(0, -3.0);
    idx.set(3, f32::NAN);
    idx.set(2, f32::INFINITY);
    assert_eq!((idx.get(0), idx.get(2), idx.get(3)), (0.0, 0.0, 0.0));
    assert_eq!(idx.total(), 7.0);
    assert_eq!(idx.find(0.0), (1, 0.0));

    let empty = HeightIndex::new(&[]);
    assert!(empty.is_empty());
    assert_eq!(empty.total(), 0.0);
    assert_eq!(empty.prefix(3), 0.0);
    assert_eq!(empty.find(5.0), (0, 0.0));

    // Against a brute-force model, including updates.
    let mut x = 7u64;
    let mut model: Vec<f32> = (0..1000).map(|_| (next(&mut x) % 50) as f32).collect();
    let mut idx = HeightIndex::new(&model);
    for _ in 0..500 {
        let i = (next(&mut x) % 1000) as usize;
        model[i] = (next(&mut x) % 50) as f32 * 0.5;
        idx.set(i, model[i]);
    }
    let mut sum = 0.0f64;
    for (i, &h) in model.iter().enumerate() {
        assert_eq!(idx.prefix(i), sum);
        if h > 0.0 {
            assert_eq!(idx.find(sum), (i, 0.0));
            assert_eq!(idx.find(sum + f64::from(h) / 2.0), (i, f64::from(h) / 2.0));
        }
        sum += f64::from(h);
    }
    assert_eq!(idx.total(), sum);
}

#[test]
fn height_index_is_exact_past_f32_precision() {
    // 29.25M px: f32 resolves only 2 px here; sums and offsets must stay exact.
    let n = 1_500_000;
    let mut idx = HeightIndex::new(&vec![19.5; n]);
    assert_eq!(idx.total(), 29_250_000.0);
    assert_eq!(idx.find(29_249_999.5), (n - 1, 19.0));
    assert_eq!(idx.prefix(n - 1), 29_249_980.5);
    // Many updates never drift.
    let mut x = 99u64;
    for k in 0..10_000 {
        let i = (next(&mut x) % n as u64) as usize;
        idx.set(i, if k % 2 == 0 { 18.25 } else { 19.5 });
    }
    for i in 0..n {
        idx.set(i, 19.5);
        if i > 20_000 {
            break;
        }
    }
    let exact: f64 = (0..n).map(|i| f64::from(idx.get(i))).sum();
    assert_eq!(idx.total(), exact);
    assert_eq!(idx.prefix(n), exact);
}

#[test]
fn height_index_100k_updates_fast() {
    let n = 100_000usize;
    let mut idx = HeightIndex::new(&vec![20.0; n]);
    let mut x = 42u64;
    let start = Instant::now();
    for k in 0..n {
        let i = (next(&mut x) % n as u64) as usize;
        idx.set(i, 10.0 + (k % 7) as f32);
    }
    let total = idx.total();
    let mut acc = 0usize;
    for _ in 0..n {
        let off = (next(&mut x) % 1_000_000) as f64 / 1_000_000.0 * total;
        acc = acc.wrapping_add(idx.find(off).0);
    }
    let elapsed = start.elapsed();
    assert!(acc > 0);
    assert!(
        elapsed < Duration::from_millis(50),
        "100k set + 100k find took {elapsed:?}"
    );
    let sum: f64 = (0..n).map(|i| f64::from(idx.get(i))).sum();
    assert_eq!(idx.total(), sum);
}

// ---------------------------------------------------------------------------
// Anchor invariants

#[test]
fn anchor_stable_when_height_above_changes() {
    let mut d = doc(20);
    d.set_viewport_height(400.0);
    for f in 0..20 {
        d.set_file_layout(f, context_layout(50, 20.0));
    }
    d.scroll_to(
        10,
        RowKey::Line {
            side: Side::New,
            line: 25,
        },
    );
    let anchor = *d.anchor();
    assert_eq!(
        anchor,
        ScrollAnchor {
            file_idx: 10,
            row: RowKey::Line {
                side: Side::New,
                line: 25
            },
            offset_px: 0.0
        }
    );
    assert_eq!(screen_y(&d, 10, 25), 0.0);
    let before = snapshot(&d, &[10, 11]);
    let top = d.scroll_top();

    // A file above grows, another shrinks, a third collapses.
    d.set_file_height(3, 5_000.0);
    // Was header 40 + 50 rows of 20.
    let grown = 5_000.0 - 1_040.0;
    assert_eq!(d.scroll_top(), top + grown);
    d.set_file_height(5, 41.0);
    d.set_collapsed(2, true);
    assert_eq!(snapshot(&d, &[10, 11]), before);
    assert_eq!(*d.anchor(), anchor);

    // Rows above the anchor inside the anchor file change height.
    d.set_row_height(10, 3, 120.0);
    d.set_row_height(10, 24, 0.0);
    assert_eq!(screen_y(&d, 10, 25), 0.0);
    assert_eq!(snapshot(&d, &[11]), before[50..].to_vec());

    // The anchor file gets more rows above the line: a block under its header
    // (blocks belong to the document, T2.7), then its layout is replaced by one
    // with different rows (the block is placed in it again).
    d.set_blocks(
        10,
        vec![PlacedBlock {
            id: BlockId(7),
            anchor: BlockAnchor::FileTop,
            height: 300.0,
        }],
    );
    assert_eq!(screen_y(&d, 10, 26), 0.0, "new 25 is body row 26 now");
    assert_eq!(snapshot(&d, &[11]), before[50..].to_vec());
    let mut layout = context_layout(50, 20.0);
    layout.set_row_height(3, 120.0);
    d.set_file_layout(10, layout);
    assert_eq!(
        d.file_layout(10).unwrap().rows()[0],
        BodyRow::Block(BlockId(7))
    );
    assert_eq!(screen_y(&d, 10, 26), 0.0);
    assert_eq!(snapshot(&d, &[11]), before[50..].to_vec());

    // Growing a file below the anchor moves nothing above it.
    let before = snapshot(&d, &[10, 11]);
    d.set_file_height(12, 9_000.0);
    assert_eq!(snapshot(&d, &[10, 11]), before);
}

#[test]
fn anchor_stable_when_estimated_becomes_exact() {
    let mut d = doc(30);
    d.set_viewport_height(600.0);
    assert!((0..30).all(|f| !d.is_exact(f)));

    // Anchor on a laid-out file; everything above goes estimated -> exact.
    d.set_file_layout(12, context_layout(80, 20.0));
    d.scroll_to(
        12,
        RowKey::Line {
            side: Side::New,
            line: 10,
        },
    );
    d.scroll_by(7.0);
    let before = snapshot(&d, &[12]);
    assert_eq!(screen_y(&d, 12, 10), -7.0);
    d.set_size_hint(
        4,
        SizeHint::Counts {
            additions: 400,
            deletions: 12,
            hunks: Some(3),
        },
    );
    for f in 0..12 {
        d.set_file_layout(f, context_layout(3 + f * 7, 20.0));
        assert!(d.is_exact(f));
    }
    assert_eq!(snapshot(&d, &[12]), before);

    // Anchor inside an estimated body: the file keeps its screen position and
    // the anchor moves onto the real row at the same pixel.
    let top20 = d.file_top(20);
    d.scroll_by((top20 - d.scroll_top()) as f32 + 40.0 + 100.0);
    assert_eq!(d.anchor().file_idx, 20);
    assert_eq!(d.anchor().row, RowKey::Placeholder);
    assert_eq!(d.anchor().offset_px, 100.0);
    let file_screen = d.file_top(20) - d.scroll_top();
    d.set_file_layout(20, context_layout(60, 20.0));
    assert_eq!(d.file_top(20) - d.scroll_top(), file_screen);
    assert_eq!(
        *d.anchor(),
        ScrollAnchor {
            file_idx: 20,
            row: RowKey::Line {
                side: Side::New,
                line: 5
            },
            offset_px: 0.0
        }
    );

    // A jump to a line of an estimated file lands exactly once it is laid out,
    // even after scrolling a little in between.
    d.scroll_to(
        25,
        RowKey::Line {
            side: Side::New,
            line: 30,
        },
    );
    d.scroll_by(15.0);
    let mut rows: Vec<BodyRow> = (0..10)
        .map(|i| BodyRow::Line {
            old: Some(i),
            new: None,
            diff_row: i,
        })
        .collect();
    rows.extend(context_layout(60, 20.0).rows().iter().copied());
    d.set_file_layout(25, FileLayout::new(rows, &[20.0; 70]));
    let row = row_showing(d.file_layout(25).unwrap(), Side::New, 30);
    assert_eq!(row, 40);
    assert_eq!(screen_y(&d, 25, row), -15.0);
}

// ---------------------------------------------------------------------------
// Window

#[test]
fn visible_range_for_offset() {
    let heights = [
        100.0, 300.0, 100.0, 100.0, 60.0, 100.0, 100.0, 100.0, 100.0, 100.0,
    ];
    let mut d = doc_with_heights(&heights, 200.0);
    assert_eq!(d.total_height(), 1_160.0);
    assert_eq!(d.scroll_top(), 0.0);
    assert_eq!(d.visible(200.0), slots(0, 2));

    d.scroll_to(1, RowKey::Header);
    assert_eq!(d.scroll_top(), 100.0);
    assert_eq!(d.visible(200.0), slots(1, 2));

    d.scroll_by(150.0);
    assert_eq!(d.scroll_top(), 250.0);
    assert_eq!(d.visible(200.0), slots(1, 3));

    // A file starting exactly at the bottom edge is not visible.
    d.scroll_to(2, RowKey::Header);
    assert_eq!(d.scroll_top(), 400.0);
    assert_eq!(d.visible(200.0), slots(2, 4));
    assert_eq!(d.visible(201.0), slots(2, 5));
    assert_eq!(d.visible(0.0), slots(2, 2));

    // Clamped at the end: the last pixel is at the viewport bottom.
    d.scroll_by(10_000.0);
    assert_eq!(d.scroll_top(), 960.0);
    assert_eq!(d.visible(200.0), slots(8, 10));
    d.scroll_by(-10_000.0);
    assert_eq!(d.scroll_top(), 0.0);

    let empty = Document::new(Arc::new(Vec::new()), metrics());
    assert_eq!(empty.visible(200.0), slots(0, 0));
    assert_eq!(empty.materialize_range(200.0, 2.0), slots(0, 0));
}

#[test]
fn materialize_window_two_screens() {
    let mut d = doc_with_heights(&[100.0; 100], 200.0);
    d.scroll_to(50, RowKey::Header);
    assert_eq!(d.scroll_top(), 5_000.0);
    assert_eq!(d.visible(200.0), slots(50, 52));
    // Two screens (400 px) above and below: [4600, 5600).
    assert_eq!(d.materialize_range(200.0, 2.0), slots(46, 56));
    assert_eq!(
        d.materialize_range(200.0, DEFAULT_WINDOW_SCREENS),
        slots(46, 56)
    );
    assert_eq!(d.materialize_range(200.0, 1.0), slots(48, 54));
    assert_eq!(d.materialize_range(200.0, 0.0), slots(50, 52));

    d.scroll_to(0, RowKey::Header);
    assert_eq!(d.materialize_range(200.0, 2.0), slots(0, 6));
    d.scroll_to(99, RowKey::Header);
    assert_eq!(d.scroll_top(), 9_800.0);
    assert_eq!(d.materialize_range(200.0, 2.0), slots(94, 100));
}

#[test]
fn evict_farthest_first_under_budget() {
    const MB: usize = 1 << 20;
    let mut d = doc_with_heights(&[100.0; 20], 200.0);
    let mut generations = Vec::new();
    for f in 0..20 {
        let generation = d.begin_loading(f);
        assert!(matches!(d.state(f), FileState::Loading { generation: g } if *g == generation));
        assert!(d.set_materialized(f, generation, materialized(10 * MB)));
        d.set_file_layout(f, context_layout(3, 20.0));
        generations.push(generation);
    }
    d.scroll_to(10, RowKey::Header);
    assert_eq!(d.visible(200.0), slots(10, 12));
    let resident = d.resident_bytes();
    assert!((200 * MB..201 * MB).contains(&resident), "{resident}");
    let heights: Vec<f32> = (0..20).map(|f| d.file_height(f)).collect();

    assert!(
        d.evict_over_budget(DEFAULT_EVICTION_BUDGET_BYTES)
            .is_empty()
    );
    let evicted = d.evict_over_budget(95 * MB);
    assert_eq!(evicted.len(), 11, "{evicted:?}");
    assert!(d.resident_bytes() <= 95 * MB);
    assert_eq!(evicted[0], 0, "file 0 is farthest");
    let distance = |f: u32| {
        let (top, bottom) = (d.file_top(f), d.file_top(f) + f64::from(d.file_height(f)));
        (1_000.0 - bottom).max(top - 1_200.0).max(0.0)
    };
    for pair in evicted.windows(2) {
        assert!(distance(pair[0]) >= distance(pair[1]), "{evicted:?}");
    }
    let kept: Vec<u32> = (0..20).filter(|f| !evicted.contains(f)).collect();
    let farthest_kept = kept.iter().map(|&f| distance(f)).fold(0.0, f64::max);
    let nearest_evicted = evicted
        .iter()
        .map(|&f| distance(f))
        .fold(f64::MAX, f64::min);
    assert!(farthest_kept <= nearest_evicted);
    for &f in &evicted {
        assert!(matches!(d.state(f), FileState::Evicted));
        assert!(d.file_layout(f).is_none());
        assert!(d.is_exact(f), "evicted files keep their exact height");
        // In-flight results of the old generation are stale now.
        assert!(!d.set_materialized(f, generations[f as usize], materialized(MB)));
    }
    let after: Vec<f32> = (0..20).map(|f| d.file_height(f)).collect();
    assert_eq!(after, heights, "eviction never changes heights");

    // The visible files are never evicted, even over budget.
    let rest = d.evict_over_budget(0);
    assert!(!rest.contains(&10) && !rest.contains(&11));
    assert!(d.state(10).is_materialized() && d.state(11).is_materialized());
    assert_eq!(d.resident_bytes() / MB, 20);
}

#[test]
fn resident_bytes_count_rows_built_on_demand() {
    let old = numbered(200).concat();
    let new = old.replace("line 100\n", "LINE 100\n");
    let file = Arc::new(MaterializedFile::from_blobs(
        Arc::from(old.as_bytes()),
        Arc::from(new.as_bytes()),
        &DiffOptions::default(),
        None,
    ));
    let mut d = doc_with_heights(&[100.0], 200.0);
    let generation = d.begin_loading(0);
    assert!(d.set_materialized(0, generation, file.clone()));
    let data = d.resident_bytes();
    assert_eq!(data, file.heap_bytes, "no rows built yet");
    // Rows are built lazily (per layout) and count toward the budget once
    // they exist.
    let rows = file.rows(Layout::Split).len() + file.rows(Layout::Unified).len();
    assert!(rows > 0);
    assert!(
        d.resident_bytes() >= data + rows * size_of::<Row>(),
        "{} < {data} + {rows} rows",
        d.resident_bytes()
    );
}

#[test]
fn collapsed_file_height_is_header_only() {
    let mut d = doc(5);
    d.set_viewport_height(300.0);
    let header = d.metrics().header_height;
    d.set_file_layout(2, context_layout(25, 20.0));
    assert_eq!(d.file_height(2), header + 500.0);
    let total = d.total_height();

    d.set_collapsed(2, true);
    assert!(d.is_collapsed(2));
    assert_eq!(d.file_height(2), header);
    assert_eq!(d.total_height(), total - 500.0);
    d.set_collapsed(2, false);
    assert_eq!(d.file_height(2), header + 500.0);

    // Estimated files collapse to the header too and get their estimate back.
    let estimate = d.file_height(3);
    assert!(estimate > header);
    d.set_collapsed(3, true);
    assert_eq!(d.file_height(3), header);
    d.set_collapsed(3, false);
    assert_eq!(d.file_height(3), estimate);

    // Collapsing the file the viewport is inside pins its header to the top.
    d.scroll_to(
        2,
        RowKey::Line {
            side: Side::New,
            line: 12,
        },
    );
    d.set_collapsed(2, true);
    assert_eq!(
        *d.anchor(),
        ScrollAnchor {
            file_idx: 2,
            row: RowKey::Header,
            offset_px: 0.0
        }
    );
    assert_eq!(d.scroll_top(), d.file_top(2));
    assert_eq!(
        d.key_offset(
            2,
            RowKey::Line {
                side: Side::New,
                line: 12
            }
        ),
        Some(0.0)
    );
}

#[test]
fn scroll_to_file_and_line() {
    let (old, new) = three_edits();
    let m = metrics();
    let header = f64::from(m.header_height);
    for layout_kind in [Layout::Unified, Layout::Split] {
        let mut d = Document::new(
            files(6),
            Metrics {
                layout: layout_kind,
                ..m.clone()
            },
        );
        d.set_viewport_height(400.0);
        let rows = rows_of(&old, &new, &Expansions::default(), layout_kind);
        d.set_file_layout(3, FileLayout::from_rows(&rows, d.metrics()));
        let layout = d.file_layout(3).unwrap().clone();
        assert_eq!(layout.len(), rows.len());

        // A changed line on each side.
        for (side, line) in [(Side::New, 50), (Side::Old, 10), (Side::Old, 90)] {
            d.scroll_to(3, RowKey::Line { side, line });
            let r = row_showing(&layout, side, line);
            assert_eq!(d.scroll_top(), d.file_top(3) + header + layout.row_top(r));
            assert_eq!(
                *d.anchor(),
                ScrollAnchor {
                    file_idx: 3,
                    row: RowKey::Line { side, line },
                    offset_px: 0.0
                },
                "{layout_kind:?}"
            );
            assert_eq!(d.visible(400.0).start, 3);
        }

        // A hidden line resolves to the gap row that hides it.
        d.scroll_to(
            3,
            RowKey::Line {
                side: Side::New,
                line: 30,
            },
        );
        let gap = layout
            .rows()
            .iter()
            .position(|r| matches!(r, BodyRow::Gap { old_start, len, .. } if (*old_start..old_start + len).contains(&30)))
            .unwrap();
        assert_eq!(d.scroll_top(), d.file_top(3) + header + layout.row_top(gap));
        d.scroll_to(3, RowKey::Gap(GapId(1)));
        assert_eq!(d.scroll_top(), d.file_top(3) + header + layout.row_top(gap));

        // The header is the top of the file.
        d.scroll_to(3, RowKey::Header);
        assert_eq!(d.scroll_top(), d.file_top(3));
        assert_eq!(d.key_offset(3, RowKey::Header), Some(0.0));
    }
}

// ---------------------------------------------------------------------------
// Beyond the card

#[test]
fn layout_toggle_keeps_line_anchor() {
    let (old, new) = three_edits();
    let split = Metrics::default();
    let mut d = Document::new(files(4), split.clone());
    d.set_viewport_height(300.0);
    let rows = rows_of(&old, &new, &Expansions::default(), Layout::Split);
    for f in 0..4 {
        d.set_file_layout(f, FileLayout::from_rows(&rows, d.metrics()));
    }
    d.scroll_to(
        2,
        RowKey::Line {
            side: Side::New,
            line: 90,
        },
    );
    d.scroll_by(5.0);
    let anchor = *d.anchor();
    assert_eq!(
        anchor.row,
        RowKey::Line {
            side: Side::New,
            line: 90
        }
    );

    let unified = Metrics {
        layout: Layout::Unified,
        ..split
    };
    d.set_metrics(unified);
    assert!((0..4).all(|f| d.file_layout(f).is_none() && !d.is_exact(f)));
    let rows = rows_of(&old, &new, &Expansions::default(), Layout::Unified);
    for f in 0..4 {
        d.set_file_layout(f, FileLayout::from_rows(&rows, d.metrics()));
    }
    assert_eq!(*d.anchor(), anchor);
    let layout = d.file_layout(2).unwrap();
    assert_eq!(screen_y(&d, 2, row_showing(layout, Side::New, 90)), -5.0);
}

#[test]
fn anchor_on_gap_survives_expansion() {
    let (old, new) = three_edits();
    let m = Metrics {
        layout: Layout::Unified,
        ..Metrics::default()
    };
    let mut d = Document::new(files(3), m);
    d.set_viewport_height(300.0);
    let rows = rows_of(&old, &new, &Expansions::default(), Layout::Unified);
    d.set_file_layout(1, FileLayout::from_rows(&rows, d.metrics()));
    // Scroll onto the gap between the first two hunks (old 14..47).
    d.scroll_to(1, RowKey::Gap(GapId(1)));
    d.scroll_by(3.0);
    let gap_start = match d.anchor().row {
        RowKey::Line {
            side: Side::Old,
            line,
        } => line,
        other => panic!("gap rows anchor by their first old line, got {other:?}"),
    };
    let file_screen = d.file_top(1) - d.scroll_top();

    // Reveal the gap's top 20 lines: the anchor lands on the first revealed line.
    let mut exp = Expansions::default();
    exp.reveal(gap_start..gap_start + 20);
    let rows = rows_of(&old, &new, &exp, Layout::Unified);
    d.set_file_layout(1, FileLayout::from_rows(&rows, d.metrics()));
    assert_eq!(d.file_top(1) - d.scroll_top(), file_screen);
    let layout = d.file_layout(1).unwrap();
    assert_eq!(
        screen_y(&d, 1, row_showing(layout, Side::Old, gap_start)),
        -3.0
    );
}

#[test]
fn scroll_by_clamps_and_keeps_pending_targets_only_inside_their_file() {
    let mut d = doc(3);
    d.set_viewport_height(100.0);
    d.scroll_by(-50.0);
    assert_eq!(d.scroll_top(), 0.0);
    assert_eq!(*d.anchor(), ScrollAnchor::default());
    d.scroll_by(f32::MAX);
    assert_eq!(d.scroll_top(), d.max_scroll());

    // A pending target in an estimated file keeps its key while scrolling
    // inside that file, and is replaced once the viewport leaves it.
    d.scroll_to(
        1,
        RowKey::Line {
            side: Side::New,
            line: 2,
        },
    );
    d.scroll_by(4.0);
    assert_eq!(
        d.anchor().row,
        RowKey::Line {
            side: Side::New,
            line: 2
        }
    );
    assert_eq!(d.anchor().offset_px, 4.0);
    d.scroll_to(1, RowKey::Header);
    d.scroll_by(d.file_height(1) + 1.0);
    assert_eq!(d.anchor().file_idx, 2);
}

#[test]
fn pending_target_below_a_pinned_header_survives_scrolling_near_the_file_top() {
    let mut d = doc(3);
    d.set_viewport_height(100.0);
    let line = RowKey::Line {
        side: Side::New,
        line: 1,
    };
    // Line 1 a header's height below the viewport's top edge, as the
    // viewport puts line targets below the pinned header: the top edge is
    // 20 px into the file, inside its header.
    d.scroll_to_anchor(ScrollAnchor {
        file_idx: 1,
        row: line,
        offset_px: -40.0,
    });
    assert_eq!(d.scroll_top(), d.file_top(1) + 20.0);
    // Anywhere below the file's top edge the target is kept, so it still
    // lands exactly once the file is laid out.
    d.scroll_by(5.0);
    assert_eq!(d.anchor().row, line);
    assert_eq!(d.anchor().offset_px, -35.0);
    d.scroll_by(-24.0);
    assert_eq!(d.anchor().row, line);
    // At the top edge itself it is the header.
    d.scroll_by(-1.0);
    assert_eq!(d.anchor().row, RowKey::Header);
    assert_eq!(d.anchor().file_idx, 1);
}

#[test]
fn stale_generations_are_rejected() {
    let mut d = doc(2);
    assert!(matches!(d.state(0), FileState::Estimated));
    let g1 = d.begin_loading(0);
    let g2 = d.begin_loading(0);
    assert!(g2 > g1);
    assert!(!d.set_materialized(0, g1, materialized(10)));
    assert!(!d.set_failed(0, g1, "stale".into()));
    assert!(matches!(d.state(0), FileState::Loading { generation } if *generation == g2));

    // Cancelling makes the in-flight generation stale.
    d.cancel_loading(0);
    assert!(matches!(d.state(0), FileState::Estimated));
    assert!(!d.set_materialized(0, g2, materialized(10)));

    let g3 = d.begin_loading(0);
    assert!(d.set_materialized(0, g3, materialized(10)));
    // A newer version (tokens swapped in) with the same generation is accepted.
    assert!(d.set_materialized(0, g3, materialized(20)));
    assert_eq!(d.resident_bytes(), 20);

    let g = d.begin_loading(1);
    assert!(d.set_failed(1, g, "blob missing".into()));
    assert!(matches!(d.state(1), FileState::Failed(m) if m == "blob missing"));
}

#[test]
fn estimates_follow_kind_and_size_hints() {
    let m = metrics();
    let header = m.header_height;
    let mut changes: Vec<FileChange> = (0..8).map(modified).collect();
    changes[1].kind = FileKind::Binary;
    changes[2].generated = true;
    changes[3].kind = FileKind::Submodule;
    changes[4].new_blob = changes[4].old_blob.clone(); // mode-only
    changes[4].new_mode = Some(Mode(0o100755));
    for added in [5, 6] {
        changes[added].status = FileStatus::Added;
        changes[added].old_path = None;
        changes[added].old_mode = None;
        changes[added].old_blob = Oid::zero(ObjectFormat::Sha1);
    }
    let mut d = Document::new(Arc::new(changes), m.clone());

    let default = d.file_height(0);
    assert!(default > header);
    assert_eq!(d.file_height(1), header + m.placeholder_height);
    assert_eq!(d.file_height(2), header + m.placeholder_height);
    assert_eq!(d.file_height(3), header + m.row_height);
    assert_eq!(d.file_height(4), header);

    // Counts refine the estimate; more changes, taller file.
    d.set_size_hint(
        0,
        SizeHint::Counts {
            additions: 5,
            deletions: 5,
            hunks: Some(1),
        },
    );
    let small = d.file_height(0);
    d.set_size_hint(
        0,
        SizeHint::Counts {
            additions: 500,
            deletions: 200,
            hunks: Some(10),
        },
    );
    let big = d.file_height(0);
    assert!(big > small && small > header);
    // Blob sizes never override counts.
    d.set_size_hint(0, SizeHint::BlobSizes { old: 10, new: 10 });
    assert_eq!(d.file_height(0), big);

    // An added file of N lines is N rows.
    d.set_size_hint(
        5,
        SizeHint::Counts {
            additions: 30,
            deletions: 0,
            hunks: Some(1),
        },
    );
    assert_eq!(d.file_height(5), header + 30.0 * m.row_height);
    // Blob sizes turn into lines for added files (32 bytes per line).
    d.set_size_hint(6, SizeHint::BlobSizes { old: 0, new: 3_200 });
    assert_eq!(d.file_height(6), header + 100.0 * m.row_height);

    // Over the "Load diff" threshold: a placeholder.
    d.set_size_hint(
        7,
        SizeHint::Counts {
            additions: 30_000,
            deletions: 0,
            hunks: Some(1),
        },
    );
    assert_eq!(d.file_height(7), header + m.placeholder_height);

    // Exact heights ignore hints.
    d.set_file_height(0, 123.0);
    d.set_size_hint(
        0,
        SizeHint::Counts {
            additions: 1,
            deletions: 1,
            hunks: None,
        },
    );
    assert_eq!(d.file_height(0), 123.0);

    // Split rows pair removed and added lines, so split is shorter than unified.
    let counts = SizeHint::Counts {
        additions: 100,
        deletions: 100,
        hunks: Some(4),
    };
    let unified = Metrics {
        layout: Layout::Unified,
        ..m.clone()
    };
    assert!(
        m.estimate_body(&modified(0), Some(counts))
            < unified.estimate_body(&modified(0), Some(counts))
    );
}

#[test]
fn file_layout_resolves_keys_and_markers() {
    let rows = vec![
        BodyRow::Gap {
            id: GapId(0),
            old_start: 0,
            new_start: 0,
            len: 10,
            diff_row: 0,
        },
        BodyRow::Line {
            old: Some(10),
            new: None,
            diff_row: 1,
        },
        BodyRow::NoNewline {
            side: Side::Old,
            diff_row: 2,
        },
        BodyRow::Line {
            old: None,
            new: Some(10),
            diff_row: 3,
        },
        BodyRow::Block(BlockId(9)),
        BodyRow::NoNewline {
            side: Side::New,
            diff_row: 4,
        },
    ];
    let layout = FileLayout::new(rows, &[32.0, 20.0, 20.0, 20.0, 100.0, 20.0]);
    assert_eq!(layout.height(), 212.0);
    assert_eq!(
        layout.find(RowKey::Line {
            side: Side::Old,
            line: 4
        }),
        Some(0)
    );
    assert_eq!(
        layout.find(RowKey::Line {
            side: Side::New,
            line: 9
        }),
        Some(0)
    );
    assert_eq!(
        layout.find(RowKey::Line {
            side: Side::Old,
            line: 10
        }),
        Some(1)
    );
    assert_eq!(
        layout.find(RowKey::Line {
            side: Side::New,
            line: 10
        }),
        Some(3)
    );
    // Past the end of a side: the nearest earlier row of that side.
    assert_eq!(
        layout.find(RowKey::Line {
            side: Side::Old,
            line: 99
        }),
        Some(1)
    );
    assert_eq!(layout.find(RowKey::Gap(GapId(0))), Some(0));
    assert_eq!(layout.find(RowKey::Gap(GapId(1))), None);
    assert_eq!(layout.find(RowKey::Block(BlockId(9))), Some(4));
    assert_eq!(layout.find(RowKey::Placeholder), None);
    assert_eq!(layout.find(RowKey::Header), None);

    assert_eq!(
        layout.key_at(5.0),
        Some((
            RowKey::Line {
                side: Side::Old,
                line: 0
            },
            5.0
        ))
    );
    assert_eq!(
        layout.key_at(40.0),
        Some((
            RowKey::Line {
                side: Side::Old,
                line: 10
            },
            8.0
        ))
    );
    // A marker row anchors to the row above it.
    assert_eq!(
        layout.key_at(60.0),
        Some((
            RowKey::Line {
                side: Side::Old,
                line: 10
            },
            28.0
        ))
    );
    assert_eq!(
        layout.key_at(150.0),
        Some((RowKey::Block(BlockId(9)), 58.0))
    );
    assert_eq!(
        layout.key_at(200.0),
        Some((RowKey::Block(BlockId(9)), 108.0))
    );
    assert_eq!(FileLayout::new(Vec::new(), &[]).key_at(0.0), None);

    let placeholder = FileLayout::placeholder(48.0);
    assert_eq!(placeholder.find(RowKey::Placeholder), Some(0));
    assert_eq!(placeholder.key_at(10.0), Some((RowKey::Placeholder, 10.0)));
}

#[test]
fn anchor_invariant_under_random_height_changes() {
    let mut x = 0x5eed_u64;
    let mut rand = |n: u64| next(&mut x) % n;
    let mut d = doc(120);
    d.set_viewport_height(500.0);
    let random_layout = |r: &mut dyn FnMut(u64) -> u64| {
        let n = r(80) as u32;
        let heights: Vec<f32> = (0..n)
            .map(|_| if r(4) == 0 { 45.0 } else { 20.0 })
            .collect();
        let rows = context_layout(n, 20.0).rows().to_vec();
        FileLayout::new(rows, &heights)
    };
    for f in 0..120 {
        match rand(3) {
            0 => d.set_file_layout(f, random_layout(&mut rand)),
            1 => d.set_file_height(f, 40.0 + rand(2_000) as f32),
            _ => {}
        }
    }
    for round in 0..300 {
        if round % 25 == 0 {
            d.scroll_to(20 + rand(60) as u32, RowKey::Header);
            d.scroll_by(rand(400) as f32 + 0.5);
        }
        let a = d.anchor().file_idx;
        let anchor = *d.anchor();
        let screen = |d: &Document| d.file_top(a) - d.scroll_top();
        let before = screen(&d);
        let f = rand(u64::from(a).max(1)) as u32;
        if f == a {
            continue;
        }
        match rand(5) {
            0 => d.set_file_height(f, 40.0 + rand(3_000) as f32),
            1 => d.set_file_layout(f, random_layout(&mut rand)),
            2 => d.set_collapsed(f, !d.is_collapsed(f)),
            3 => d.set_size_hint(
                f,
                SizeHint::Counts {
                    additions: rand(900) as u32,
                    deletions: rand(900) as u32,
                    hunks: None,
                },
            ),
            _ => {
                // Rows above the anchor row inside the anchor file.
                let row = d
                    .file_layout(a)
                    .and_then(|l| l.find(anchor.row))
                    .filter(|&r| r > 0);
                if let Some(r) = row {
                    let target = rand(r as u64) as u32;
                    d.set_row_height(a, target, rand(200) as f32);
                    let layout = d.file_layout(a).unwrap();
                    let y = d.file_top(a)
                        + f64::from(d.metrics().header_height)
                        + layout.row_top(r)
                        + f64::from(anchor.offset_px)
                        - d.scroll_top();
                    assert_eq!(y, 0.0, "round {round}: anchor row moved");
                    assert_eq!(*d.anchor(), anchor);
                    continue;
                }
                d.set_file_height(f, 41.0);
            }
        }
        assert_eq!(
            screen(&d),
            before,
            "round {round}: file {f} above moved file {a}"
        );
        assert_eq!(*d.anchor(), anchor, "round {round}");
        // Changes below the anchor never move it either.
        let below = a + 1 + rand(u64::from(119 - a).max(1)) as u32;
        if below < 120 {
            d.set_file_height(below, 40.0 + rand(3_000) as f32);
            assert_eq!(
                screen(&d),
                before,
                "round {round}: file {below} below moved {a}"
            );
        }
    }
}

#[test]
fn restoring_a_line_anchor_into_a_collapsed_file_pins_its_header() {
    let mut d = doc(4);
    d.set_viewport_height(200.0);
    d.set_file_layout(1, context_layout(40, 20.0));
    d.set_collapsed(1, true);
    d.scroll_to(
        1,
        RowKey::Line {
            side: Side::New,
            line: 30,
        },
    );
    assert_eq!(
        *d.anchor(),
        ScrollAnchor {
            file_idx: 1,
            row: RowKey::Header,
            offset_px: 0.0
        }
    );
    // Expanding keeps the header where it is; the body appears below it.
    d.set_collapsed(1, false);
    assert_eq!(d.scroll_top(), d.file_top(1));
}

#[test]
fn file_layout_of_200k_rows_is_fast() {
    // A 200k-line file fully expanded: build, then 20k key lookups each way.
    let start = Instant::now();
    let layout = context_layout(200_000, 20.0);
    assert_eq!(layout.height(), 4_000_000.0);
    let mut x = 3u64;
    for _ in 0..20_000 {
        let line = (next(&mut x) % 200_000) as u32;
        let key = RowKey::Line {
            side: Side::New,
            line,
        };
        let row = layout.find(key).unwrap();
        assert_eq!(row, line as usize);
        assert_eq!(layout.key_at(layout.row_top(row) + 3.0), Some((key, 3.0)));
    }
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
}

#[test]
fn eviction_can_keep_the_materialization_window() {
    const MB: usize = 1 << 20;
    let mut d = doc_with_heights(&[100.0; 20], 200.0);
    for f in 0..20 {
        let generation = d.begin_loading(f);
        assert!(d.set_materialized(f, generation, materialized(10 * MB)));
    }
    d.scroll_to(10, RowKey::Header);
    // Everything but the window 8..14 goes, even with no budget at all.
    let evicted = d.evict_over_budget_keeping(0, slots(8, 14));
    let kept: Vec<u32> = (0..20).filter(|&f| d.state(f).is_materialized()).collect();
    assert_eq!(kept, (8..14).collect::<Vec<u32>>());
    assert_eq!(evicted.len(), 14);
    assert_eq!(evicted[0], 0, "farthest first");
    // The visible files are kept even when `keep` misses them.
    d.evict_over_budget_keeping(0, slots(0, 0));
    assert!(d.state(10).is_materialized() && d.state(11).is_materialized());
    assert!(!d.state(8).is_materialized());
}

#[test]
fn set_kind_reestimates_unless_exact() {
    let mut d = doc(3);
    d.set_viewport_height(100.0);
    let m = d.metrics().clone();
    let text = m.header_height + m.default_body_rows as f32 * m.row_height;
    assert_eq!(d.file_height(1), text);
    // An estimated text file that turns out binary becomes a placeholder.
    d.set_kind(1, FileKind::Binary);
    assert_eq!(d.files()[1].kind, FileKind::Binary);
    assert_eq!(d.file_height(1), m.header_height + m.placeholder_height);
    // An exact height stays as it is.
    d.set_file_height(2, 77.0);
    d.set_kind(2, FileKind::Binary);
    assert_eq!(d.files()[2].kind, FileKind::Binary);
    assert_eq!(d.file_height(2), 77.0);
    // The list is copied on write: the caller's shared list is untouched.
    let shared = files(3);
    let mut d = Document::new(shared.clone(), metrics());
    d.set_kind(0, FileKind::Binary);
    assert_eq!(shared[0].kind, FileKind::Text);
    assert_eq!(d.files()[0].kind, FileKind::Binary);
}

// ---------------------------------------------------------------------------
// cards (T6.5): leads, padding, the prelude

/// Metrics with the cards' gap (12) and padding (8): header 40, rows 20.
fn card_metrics() -> Metrics {
    Metrics {
        card_gap: 12.0,
        card_pad_bottom: 8.0,
        ..metrics()
    }
}

/// Four files on cards: 0 has 5 rows (100 px), 1 an empty body, 2 is
/// collapsed, 3 (the last) has 3 rows (60 px).
fn card_doc() -> Document {
    let mut d = Document::new(files(4), card_metrics());
    d.set_viewport_height(200.0);
    d.set_file_layout(0, context_layout(5, 20.0));
    d.set_file_height(1, 40.0);
    d.set_collapsed(2, true);
    d.set_file_layout(3, context_layout(3, 20.0));
    d
}

#[test]
fn lead_and_pad_shape_file_heights() {
    let d = card_doc();
    // No prelude: the first file has no lead; padding only under a body;
    // the last file holds the gap below its card.
    assert_eq!(
        (0..4).map(|f| d.file_height(f)).collect::<Vec<_>>(),
        [
            40.0 + 100.0 + 8.0,
            12.0 + 40.0,
            12.0 + 40.0,
            12.0 + 40.0 + 60.0 + 8.0 + 12.0
        ]
    );
    assert_eq!(d.total_height(), 148.0 + 52.0 + 52.0 + 132.0);
    assert_eq!((d.lead(0), d.lead(1), d.lead(3)), (0.0, 12.0, 12.0));
    assert_eq!(
        (d.header_top(0), d.body_top(0), d.card_bottom(0)),
        (0.0, 40.0, 148.0)
    );
    assert_eq!(
        (d.header_top(1), d.body_top(1), d.card_bottom(1)),
        (160.0, 200.0, 200.0)
    );
    assert_eq!((d.header_top(2), d.card_bottom(2)), (212.0, 252.0));
    assert_eq!(
        (d.header_top(3), d.body_top(3), d.card_bottom(3)),
        (264.0, 304.0, 372.0)
    );
    assert_eq!(d.body_height(2), 0.0);

    // A 70 px prelude is the first file's lead, with a gap below it.
    let mut d = d;
    d.set_prelude_height(Some(70.0));
    assert_eq!(d.lead(0), 82.0);
    assert_eq!(d.file_height(0), 82.0 + 40.0 + 100.0 + 8.0);
    assert_eq!((d.header_top(0), d.body_top(0)), (82.0, 122.0));
    assert_eq!(d.header_top(1), 230.0 + 12.0);
    d.set_prelude_height(None);
    assert_eq!(d.file_height(0), 148.0);

    // The flat layout is unchanged: no leads, no padding.
    let mut flat = Document::new(files(2), metrics());
    flat.set_file_layout(0, context_layout(5, 20.0));
    flat.set_file_layout(1, context_layout(3, 20.0));
    assert_eq!((flat.file_height(0), flat.file_height(1)), (140.0, 100.0));
    assert_eq!(flat.header_top(1), 140.0);
}

#[test]
fn anchor_in_the_gap_above_a_card_uses_the_lead_key_and_survives_height_changes() {
    let mut d = card_doc();
    // 2 px into the canvas above file 1's card (it starts at 148).
    d.scroll_by(150.0);
    assert_eq!(
        *d.anchor(),
        ScrollAnchor {
            file_idx: 1,
            row: RowKey::Lead,
            offset_px: 2.0,
        }
    );
    assert_eq!(d.key_offset(1, RowKey::Lead), Some(0.0));
    assert_eq!(d.key_offset(1, RowKey::Header), Some(12.0));
    // File 0 grows above it (5 → 10 rows): the canvas stays at the top.
    d.set_file_layout(0, context_layout(10, 20.0));
    assert_eq!(d.file_top(1), 248.0);
    assert_eq!(d.scroll_top(), 250.0);
    // A prelude appears above everything: still the same place.
    d.set_prelude_height(Some(70.0));
    assert_eq!(d.scroll_top(), 82.0 + 248.0 + 2.0);
    assert_eq!(d.anchor().row, RowKey::Lead);
    // Collapsing the file keeps a lead anchor (it is above the body).
    d.set_collapsed(1, true);
    assert_eq!(d.anchor().row, RowKey::Lead);
    // Its header at the top: a header anchor, the lead above the viewport
    // (files: 330, 52, 52, then 12 px of canvas above file 3's card).
    d.set_viewport_height(50.0);
    d.scroll_to(3, RowKey::Header);
    assert_eq!(d.scroll_top(), 330.0 + 52.0 + 52.0 + 12.0);
    // Scrolled up into the lead above the card: the lead key again.
    d.scroll_by(-5.0);
    assert_eq!((d.anchor().file_idx, d.anchor().row), (3, RowKey::Lead));
    assert_eq!(d.anchor().offset_px, 7.0);
}

#[test]
fn the_top_of_the_document_is_the_first_lead_and_stays_at_the_top() {
    let mut d = card_doc();
    assert_eq!(*d.anchor(), ScrollAnchor::default());
    assert_eq!(ScrollAnchor::default().row, RowKey::Lead);
    // A prelude set late, then growing: the top stays the top.
    d.set_prelude_height(Some(70.0));
    assert_eq!(d.scroll_top(), 0.0);
    d.set_prelude_height(Some(400.0));
    assert_eq!(d.scroll_top(), 0.0);
    // Scrolling down and back to 0 is the top again, not file 0's header.
    d.set_prelude_height(None);
    d.scroll_by(30.0);
    d.scroll_by(-30.0);
    assert_eq!(*d.anchor(), ScrollAnchor::default());
    d.set_prelude_height(Some(70.0));
    assert_eq!(d.scroll_top(), 0.0);
}

#[test]
fn top_line_is_the_first_line_below_the_pinned_header_with_cards() {
    let mut d = Document::new(files(2), card_metrics());
    d.set_viewport_height(200.0);
    d.set_prelude_height(Some(70.0));
    d.set_file_layout(0, context_layout(10, 20.0));
    d.set_file_layout(1, context_layout(10, 20.0));
    // At the top: the prelude and file 0's header in view, its first line.
    assert_eq!(d.top_line(), Some((0, Side::New, 0)));
    // File 0's body (122..322) under its pinned header: line 4's row starts
    // 80 px into it.
    d.scroll_to_anchor(ScrollAnchor {
        file_idx: 0,
        row: RowKey::Line {
            side: Side::New,
            line: 4,
        },
        offset_px: -40.0,
    });
    assert_eq!(d.scroll_top(), 122.0 + 80.0 - 40.0);
    assert_eq!(d.top_line(), Some((0, Side::New, 4)));
    // The pixel below the pinned header in file 0's padding (322..330):
    // nothing of file 0's body shows, then come the canvas and file 1's
    // card, so it is file 1's first line.
    d.scroll_by(123.0);
    assert_eq!(d.scroll_top() + 40.0, 325.0);
    assert_eq!(d.top_line(), Some((1, Side::New, 0)));
}
