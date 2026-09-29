//! Background materialization pipeline (T2.6, design §12.4, §11.11, §6.3):
//! priorities, cancellation, token swap-in, background counts, eviction, the
//! 100k-line syntax cutoff and binary detection on first read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, FileKind, ObjectFormat, Oid, Side};
use polygloss_viewport::{
    DiffProvider, DiffViewport, FileCounts, FileState, LayoutMode, LoadError, LoadOptions, Loaded,
    MaterializedFile, ScrollTarget, ViewportEvent,
};

use crate::support::*;

/// `n` added `.txt` files of exactly 20 lines: 20 rows is also the estimate
/// for a file nothing is known about, so loading them never changes a height
/// (every file is `HEADER_H + 20 × ROW_H` = 440 px tall from the start).
fn twenty_line_files(n: usize) -> Vec<Spec> {
    (0..n)
        .map(|i| Spec::added(&format!("f{i:02}.txt"), &numbered("line", 20).concat()))
        .collect()
}

const FILE_H: f64 = 440.0;

fn state(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, f: u32) -> FileState {
    view.read_with(cx, |v, _| v.document().state(f).clone())
}

fn counts(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, f: u32) -> Option<FileCounts> {
    view.read_with(cx, |v, _| v.file_counts(f))
}

#[gpui_kit::test]
fn materializes_visible_files_first(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(30));
    // The first frame is drawn at file 20, which starts at 20 × 440 = 8800:
    // files 20 and 21 are visible (8800..9400) and the ±2-screen window
    // (7600..10600) spans 17..=24.
    let (view, cx) = open_idle_at(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
        ScrollTarget::File(20),
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.document().scroll_top()),
        20.0 * FILE_H
    );
    cx.run_until_parked();
    // Visible files first, top to bottom; then the rest of the window by
    // distance from the viewport: 19 (0 px above), 22 (280 px below), 18
    // (440 above), 23 (720 below), 17 (880 above), 24 (1160 below). Only then
    // does the background pass (started by the first frame that showed every
    // visible row) read the other files.
    let order = provider.load_order();
    assert_eq!(order[..8], [20, 21, 19, 22, 18, 23, 17, 24], "{order:?}");
    for f in 17..=24 {
        assert!(state(&view, cx, f).is_materialized(), "file {f}");
    }
    // The pass only counts: files outside the window stay unloaded.
    assert!(matches!(state(&view, cx, 16), FileState::Estimated));
    assert!(matches!(state(&view, cx, 25), FileState::Estimated));
}

#[gpui_kit::test]
fn scrolled_away_work_is_cancelled(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(60));
    let (view, cx) = open_idle(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    // The first frame (drawn when the window opens) queued the window at the
    // top (0..1200: files 0, 1, 2)…
    for f in 0..3 {
        assert!(
            matches!(state(&view, cx, f), FileState::Loading { .. }),
            "file {f}"
        );
    }
    // …then the user jumps away before any of it ran.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(40), cx));
    redraw(cx);
    for f in 0..3 {
        assert!(
            matches!(state(&view, cx, f), FileState::Estimated),
            "file {f}: {:?}",
            state(&view, cx, f)
        );
    }
    assert_eq!(pipeline_stats(&view, cx).cancelled, 3);

    cx.run_until_parked();
    // The new window was read first (visible file 40, then by distance)…
    let order = provider.load_order();
    assert_eq!(order[..5], [40, 39, 41, 38, 42], "{order:?}");
    // …and the cancelled loads never ran: files 0..3 (one blob each, added
    // files) were read once, by the background counts pass, and stay
    // unloaded.
    for f in 0..3 {
        assert_eq!(provider.loads_of(f), 1, "file {f}");
        assert!(matches!(state(&view, cx, f as u32), FileState::Estimated));
    }
    assert_eq!(pipeline_stats(&view, cx).cancelled, 3);
}

/// Wraps a provider and raises `cancel` while the first blob is read, like
/// the main thread cancelling a load that is already running.
struct CancelDuringRead {
    inner: Arc<MemProvider>,
    cancel: Arc<AtomicUsize>,
}

impl DiffProvider for CancelDuringRead {
    fn object_format(&self) -> ObjectFormat {
        self.inner.object_format()
    }

    fn files(&self) -> Arc<Vec<FileChange>> {
        self.inner.files()
    }

    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        self.cancel.store(1, Ordering::SeqCst);
        self.inner.load_blob(oid)
    }

    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        self.inner.blob_size(oid)
    }
}

#[test]
fn running_load_stops_at_its_next_stage_when_cancelled() {
    let inner = MemProvider::new(vec![Spec::modified("a.rs", "a\nb\n", "a\nc\n")]);
    let cancel = Arc::new(AtomicUsize::new(0));
    let provider = CancelDuringRead {
        inner: inner.clone(),
        cancel: cancel.clone(),
    };
    let change = inner.files()[0].clone();
    let opts = LoadOptions {
        rows: Some(Layout::Split),
        ..LoadOptions::default()
    };
    let result = MaterializedFile::load(&provider, &change, &opts, &cancel);
    assert!(matches!(result, Err(LoadError::Cancelled)), "{result:?}");
    // It stopped right after the first read: the new blob was never read.
    assert_eq!(inner.load_count(), 1);

    // Not cancelled: both blobs, the diff, word ranges and the rows.
    cancel.store(0, Ordering::SeqCst);
    let never = AtomicUsize::new(0);
    match MaterializedFile::load(&*inner, &change, &opts, &never) {
        Ok(Loaded::File(file)) => {
            assert_eq!((file.diff.additions, file.diff.deletions), (1, 1));
            assert_eq!(file.words.len(), 1);
            assert!(file.rows_split.get().is_some(), "rows built in the load");
        }
        other => panic!("{other:?}"),
    }
}

#[gpui_kit::test]
fn tokens_swap_in_without_moving_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let src = |p: &str| {
        (0..60)
            .map(|i| format!("fn {p}{i}() {{ let x = {i}; }}\n"))
            .collect::<String>()
    };
    let provider = MemProvider::new(vec![
        Spec::added("src/a.rs", &src("a")),
        Spec::added("src/b.rs", &src("b")),
    ]);
    let mut opts = options(LayoutMode::Split);
    opts.syntax = false;
    let (view, cx) = open(cx, provider, opts, 1300., 400.);
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 1,
                side: Side::New,
                line: 30,
            },
            cx,
        )
    });
    settle(cx);
    let plain = debug(&view, cx);
    let total = view.read_with(cx, |v, _| v.document().total_height());
    assert_eq!(plain.styled_rows, 0);
    let (events, _sub) = record_events(&view, cx);

    // Syntax on: the frame right after paints plain text and counts the rows
    // as waiting for tokens.
    view.update(cx, |v, cx| {
        let mut o = v.options().clone();
        o.syntax = true;
        v.set_options(o, cx)
    });
    redraw(cx);
    let waiting = last_stats(&events);
    assert_eq!(waiting.unhighlighted_rows, 20, "{waiting:?}");
    assert_eq!(debug(&view, cx).styled_rows, 0);

    // Tokens swap in: same rows, same places, same anchor, same heights.
    settle(cx);
    let styled = debug(&view, cx);
    assert_eq!(styled.styled_rows, 20);
    assert_eq!(styled.visible_rows, plain.visible_rows);
    assert_eq!(styled.row_bounds, plain.row_bounds);
    assert_eq!(styled.anchor, plain.anchor);
    assert_eq!(
        view.read_with(cx, |v, _| v.document().total_height()),
        total
    );
    assert_eq!(last_stats(&events).unhighlighted_rows, 0);
    // The file above the viewport is in the window: it got tokens too.
    match state(&view, cx, 0) {
        FileState::Materialized(f) => assert!(f.new_tokens.is_some()),
        other => panic!("{other:?}"),
    }
}

/// `n` numbered lines with lines `changed` replaced.
fn with_changes(n: u32, changed: &[u32]) -> (String, String) {
    let old = numbered("line", n);
    let mut new = old.clone();
    for &i in changed {
        new[i as usize] = format!("LINE {i}\n");
    }
    (old.concat(), new.concat())
}

#[gpui_kit::test]
fn counts_fill_progressively_after_first_paint(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // File i changes `i % 4 + 1` lines, 10 lines apart.
    let specs: Vec<Spec> = (0..50)
        .map(|i| {
            let changed: Vec<u32> = (0..=(i % 4)).map(|k| 2 + 10 * k).collect();
            let (old, new) = with_changes(40, &changed);
            Spec::modified(&format!("src/f{i:02}.txt"), &old, &new)
        })
        .collect();
    let expected = |i: u32| FileCounts {
        additions: i % 4 + 1,
        deletions: i % 4 + 1,
    };
    let provider = MemProvider::new(specs);
    let (view, cx) = open_idle(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    // Log the first frame that painted every visible row among the reads.
    let log = provider.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &ViewportEvent, _| {
            if let ViewportEvent::FrameStats(s) = e
                && s.loading_rows == 0
                && !log.load_order().contains(&MARK)
            {
                log.mark();
            }
        })
    });
    let far_h = view.read_with(cx, |v, _| v.document().file_height(49));
    let anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(counts(&view, cx, 49), None);

    settle(cx);
    let order = provider.load_order();
    let painted = order
        .iter()
        .position(|&f| f == MARK)
        .expect("a frame showed every visible row");
    // Before first paint only the window at the top was read (the visible
    // files first); the counts of every other file came after it.
    let window = &order[..painted];
    assert_eq!(window[..4], [0, 0, 1, 1], "{order:?}");
    assert!(window.iter().all(|&f| f < 8), "{order:?}");
    for f in 0..50 {
        assert_eq!(counts(&view, cx, f), Some(expected(f)), "file {f}");
    }
    // Each file far below was read once, by the counts pass; the visible
    // files, counted by their load before first paint, were not read again.
    for f in 8..50 {
        assert_eq!(provider.loads_of(f), 2, "file {f}");
        assert!(!window.contains(&f));
    }
    for f in 0..2 {
        assert_eq!(provider.loads_of(f), 2, "file {f}");
    }
    // Estimates of files far below were refined; nothing on screen moved.
    let refined = view.read_with(cx, |v, _| v.document().file_height(49));
    assert_ne!(refined, far_h);
    assert_eq!(view.read_with(cx, |v, _| v.anchor()), anchor);
}

#[gpui_kit::test]
fn eviction_drops_rows_and_tokens_keeps_metadata(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let specs: Vec<Spec> = (0..20)
        .map(|i| {
            let old: String = (0..30).map(|k| format!("fn f{k}() {{}}\n")).collect();
            let new = old.replace("fn f5()", "fn g5()");
            Spec::modified(&format!("src/m{i:02}.rs"), &old, &new)
        })
        .collect();
    let provider = MemProvider::new(specs);
    let mut opts = options(LayoutMode::Unified);
    // Keep only the visible files, and evict everything else at once.
    opts.window_screens = 0.0;
    opts.eviction_budget_bytes = 1;
    let (view, cx) = open(cx, provider.clone(), opts, 1000., 400.);
    match state(&view, cx, 0) {
        FileState::Materialized(f) => {
            assert!(f.new_tokens.is_some() && f.old_tokens.is_some());
            assert!(f.rows_unified.get().is_some());
        }
        other => panic!("{other:?}"),
    }
    let height = view.read_with(cx, |v, _| v.document().file_height(0));
    let counts0 = counts(&view, cx, 0).expect("counts from the load");
    let path = view.read_with(cx, |v, _| v.document().files()[0].display_path().to_owned());

    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(15), cx));
    settle(cx);
    // File 0 left the window: its data, rows and tokens are gone…
    assert!(matches!(state(&view, cx, 0), FileState::Evicted));
    view.read_with(cx, |v, _| {
        let d = v.document();
        assert!(d.file_layout(0).is_none());
        // …its exact height, counts and metadata are kept.
        assert!(d.is_exact(0));
        assert_eq!(d.file_height(0), height);
        assert_eq!(d.files()[0].display_path(), path);
        // Only visible files hold data.
        let visible = d.visible(d.viewport_height());
        for f in 0..d.len() {
            assert_eq!(
                d.state(f).is_materialized(),
                visible.contains(&f),
                "file {f}"
            );
        }
    });
    assert_eq!(counts(&view, cx, 0), Some(counts0));

    // Back at the top the file is read again; its tokens come from the token
    // cache instead of another highlight.
    let before = pipeline_stats(&view, cx);
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    settle(cx);
    let after = pipeline_stats(&view, cx);
    assert_eq!(after.highlights, before.highlights, "{after:?}");
    assert!(after.token_cache_hits >= before.token_cache_hits + 2);
    match state(&view, cx, 0) {
        FileState::Materialized(f) => assert!(f.new_tokens.is_some()),
        other => panic!("{other:?}"),
    }
    assert!(debug(&view, cx).styled_rows > 0);
    assert_eq!(provider.loads_of(0), 4, "read twice");
}

#[gpui_kit::test]
fn file_over_100k_lines_skips_syntax_until_requested(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // 100,001 lines per side, one changed in the middle.
    let old = "1\n".repeat(100_001);
    let new = format!("{}2\n{}", "1\n".repeat(50_000), "1\n".repeat(50_000));
    let provider = MemProvider::new(vec![Spec::modified("big.py", &old, &new)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let (events, _sub) = record_events(&view, cx);
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 49_998,
            },
            cx,
        )
    });
    settle(cx);
    let d = debug(&view, cx);
    assert!(d.visible_rows.iter().any(|r| r.ends_with("+ 2")), "{d:?}");
    assert_eq!(d.styled_rows, 0);
    // Plain on purpose, not waiting for tokens: highlight_ms is not held up.
    assert_eq!(last_stats(&events).unhighlighted_rows, 0);
    assert!(view.read_with(cx, |v, _| v.syntax_skipped(0)));
    assert_eq!(pipeline_stats(&view, cx).highlights, 0);

    view.update(cx, |v, cx| v.highlight_anyway(0, cx));
    settle(cx);
    assert!(!view.read_with(cx, |v, _| v.syntax_skipped(0)));
    assert!(debug(&view, cx).styled_rows > 0);
    assert_eq!(last_stats(&events).unhighlighted_rows, 0);
}

#[gpui_kit::test]
fn binary_detected_on_first_read_sets_kind(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Both NUL files are listed as text (no attribute said otherwise): the
    // first is visible, the last is far below and found by the background
    // pass.
    let mut specs = vec![Spec::modified("data.bin", "a\0b\n", "a\0c\n")];
    specs.extend(twenty_line_files(20));
    specs.push(Spec::modified("far.dat", "x\n\0\n", "y\n\0\n"));
    let provider = MemProvider::new(specs);
    assert_eq!(provider.files()[0].kind, FileKind::Text);
    let (view, cx) = open_idle(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    let (events, _sub) = record_events(&view, cx);
    settle(cx);

    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[..2], ["== data.bin", "Binary file"]);
    assert_eq!(d.row_bounds[1], (HEADER_H, PLACEHOLDER_H));
    view.read_with(cx, |v, _| {
        let doc = v.document();
        assert_eq!(doc.files()[0].kind, FileKind::Binary);
        assert_eq!(doc.files()[21].kind, FileKind::Binary);
        assert!(!doc.state(0).is_materialized());
        assert!(doc.is_exact(0));
    });
    let detected: Vec<u32> = events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            ViewportEvent::BinaryDetected(f) => Some(*f),
            _ => None,
        })
        .collect();
    assert_eq!(detected, vec![0, 21]);
    // Each was read once, up to its old side (binary already), and never
    // again.
    assert_eq!(provider.loads_of(0), 1);
    assert_eq!(provider.loads_of(21), 1);
    assert_eq!(counts(&view, cx, 0), None);
}

#[gpui_kit::test]
fn counts_survive_loads_cancelled_during_the_pass(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(64));
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    // New diff options restart the counts; then the user jumps around while
    // the pass runs, so loads that would have counted files are queued and
    // cancelled around it. Every file still ends up counted.
    view.update(cx, |v, cx| {
        let mut o = v.options().clone();
        o.diff.ignore_whitespace = true;
        v.set_options(o, cx)
    });
    for target in [20, 50, 3, 60, 33, 12, 45, 27, 8, 56] {
        view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(target), cx));
        redraw(cx);
        for _ in 0..3 {
            cx.executor().tick();
        }
    }
    settle(cx);
    for f in 0..64 {
        assert_eq!(
            counts(&view, cx, f),
            Some(FileCounts {
                additions: 20,
                deletions: 0
            }),
            "file {f}"
        );
    }
    assert!(pipeline_stats(&view, cx).cancelled > 0);
}

/// Wraps a provider and appends a line that is not UTF-8 to every blob:
/// lumis refuses such text (`Unsupported`). It knows no blob sizes, so the
/// background pass has nothing to repaint for.
struct NotUtf8(Arc<MemProvider>);

impl DiffProvider for NotUtf8 {
    fn object_format(&self) -> ObjectFormat {
        self.0.object_format()
    }

    fn files(&self) -> Arc<Vec<FileChange>> {
        self.0.files()
    }

    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        let mut bytes = self.0.load_blob(oid)?.to_vec();
        bytes.extend_from_slice(b"\xff\n");
        Ok(bytes.into())
    }

    fn blob_size(&self, _oid: &Oid) -> anyhow::Result<u64> {
        anyhow::bail!("no sizes")
    }
}

#[gpui_kit::test]
fn highlight_without_tokens_stops_counting_rows(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let inner = MemProvider::new(vec![Spec::added("src/a.rs", "fn a() {}\nfn b() {}\n")]);
    let provider: Arc<dyn DiffProvider> = Arc::new(NotUtf8(inner));
    let opts = options(LayoutMode::Unified);
    let window = cx.open_window(gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(400.)), {
        move |window, cx| DiffViewport::new(provider, opts, window, cx)
    });
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    let (events, _sub) = record_events(&view, cx);
    // Only background work, no explicit redraw: the frames that follow come
    // from the results themselves.
    cx.run_until_parked();
    assert_eq!(
        pipeline_stats(&view, cx).highlights,
        1,
        "the new side was tried"
    );
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows.len(), 4, "{:?}", d.visible_rows);
    assert_eq!(d.styled_rows, 0);
    // The side will never get tokens: its rows are not waiting for any.
    let stats = last_stats(&events);
    assert_eq!((stats.loading_rows, stats.unhighlighted_rows), (0, 0));
}
