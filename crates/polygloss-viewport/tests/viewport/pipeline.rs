//! Background materialization pipeline (T2.6, design §12.4, §11.11, §6.3):
//! priorities, cancellation, token swap-in, background counts, eviction, the
//! 100k-line syntax cutoff and binary detection on first read.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, FileKind, ObjectFormat, Oid, Side};
use polygloss_highlight::{
    Appearance, Budget, Highlighter, Rgba, SyntaxStyle, Tokens, guess_language, pierre_theme,
};
use polygloss_viewport::{
    DiffProvider, DiffViewport, FileCounts, FileState, LayoutMode, LoadError, LoadOptions, Loaded,
    MaterializedFile, ScrollTarget, SlotRange, ViewportEvent, ViewportTheme,
};

use crate::support::*;

/// `n` added `.txt` files of exactly 20 lines: 20 rows is also the estimate
/// for a file nothing is known about, so loading them never changes a height
/// (every file is `HEADER_H + 20 × ROW_H` = 445 px tall from the start).
fn twenty_line_files(n: usize) -> Vec<Spec> {
    (0..n)
        .map(|i| Spec::added(&format!("f{i:02}.txt"), &numbered("line", 20).concat()))
        .collect()
}

const FILE_H: f64 = 445.0;

/// `n` modified Rust files of 30 functions each, one renamed.
fn rust_files(n: usize) -> Vec<Spec> {
    (0..n)
        .map(|i| {
            let old: String = (0..30).map(|k| format!("fn f{k}() {{}}\n")).collect();
            let new = old.replace("fn f5()", "fn g5()");
            Spec::modified(&format!("src/m{i:02}.rs"), &old, &new)
        })
        .collect()
}

/// Pierre Light with one more `syntax` key that sorts first: every style id
/// of Pierre Light means the next style here, like a user theme whose keys
/// differ from the one it replaces.
fn shifted_theme() -> Arc<ViewportTheme> {
    let mut t = pierre_theme(Appearance::Light).clone();
    t.name = "Pierre Light (shifted)".into();
    t.syntax.insert(
        "aaa".into(),
        SyntaxStyle {
            color: Rgba::parse("#ff0000"),
            ..SyntaxStyle::default()
        },
    );
    Arc::new(ViewportTheme::from_zed(&t))
}

/// What highlighting `text` as Rust with `theme` gives.
fn rust_tokens(theme: &ViewportTheme, text: &[u8]) -> Tokens {
    let rust = guess_language("a.rs", text).expect("rust grammar");
    Highlighter::new(theme.syntax.clone())
        .highlight(text, &rust, &AtomicUsize::new(0), Budget::default())
        .expect("highlights")
}

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
    // The first frame is drawn at file 20, which starts at 20 × 445 = 8900:
    // files 20 and 21 are visible (8900..9500) and the ±2-screen window
    // (7700..10700) spans 17..=24.
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
    // distance from the viewport: 19 (0 px above), 22 (290 px below), 18
    // (445 above), 23 (735 below), 17 (890 above), 24 (1180 below). Only then
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
    // 21 code rows: line 30 and the 17.75 below it, and the 2.25 the 45 px
    // pinned header covers above it.
    let waiting = last_stats(&events);
    assert_eq!(waiting.unhighlighted_rows, 21, "{waiting:?}");
    assert_eq!(debug(&view, cx).styled_rows, 0);

    // Tokens swap in: same rows, same places, same anchor, same heights.
    settle(cx);
    let styled = debug(&view, cx);
    assert_eq!(styled.styled_rows, 21);
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

    // Run the background work one task at a time: after first paint the
    // counts arrive a few files at a time, not all at once at the end.
    let known = |cx: &mut VisualTestContext| {
        (0..50)
            .filter(|&f| view.read_with(cx, |v, _| v.file_counts(f)).is_some())
            .count()
    };
    let mut after_paint = Vec::new();
    loop {
        if provider.load_order().contains(&MARK) {
            let k = known(cx);
            if after_paint.last() != Some(&k) {
                after_paint.push(k);
            }
        }
        if !cx.executor().tick() {
            break;
        }
    }
    assert_eq!(after_paint.last(), Some(&50), "{after_paint:?}");
    assert!(after_paint[0] < 50 - 16, "{after_paint:?}");
    assert!(
        after_paint.len() >= 3 && after_paint.windows(2).all(|w| w[0] < w[1]),
        "{after_paint:?}"
    );

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
    let provider = MemProvider::new(rust_files(20));
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
                d.contains_file(visible, f),
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
fn theme_change_during_a_cached_load_drops_the_old_tokens(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(rust_files(20));
    let mut opts = options(LayoutMode::Unified);
    // Only visible files are kept, everything else is evicted at once.
    opts.window_screens = 0.0;
    opts.eviction_budget_bytes = 1;
    let light = opts.theme.clone();
    let shifted = shifted_theme();
    assert_ne!(light.syntax_id(), shifted.syntax_id());
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    // File 0 was highlighted with Pierre Light (and its tokens cached), then
    // evicted.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(15), cx));
    settle(cx);
    assert!(matches!(state(&view, cx, 0), FileState::Evicted));

    // Back at the top, file 0's load is queued: it will attach the cached
    // Pierre Light tokens. The theme changes before it runs.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    redraw(cx);
    assert!(matches!(state(&view, cx, 0), FileState::Loading { .. }));
    let highlights = pipeline_stats(&view, cx).highlights;
    view.update(cx, |v, cx| {
        let mut o = v.options().clone();
        o.theme = shifted.clone();
        v.set_options(o, cx)
    });
    settle(cx);

    // The file shows tokens of the new theme, highlighted again for it.
    let file = match state(&view, cx, 0) {
        FileState::Materialized(f) => f,
        other => panic!("{other:?}"),
    };
    let tokens = file.new_tokens.as_deref().expect("new side highlighted");
    assert!(
        *tokens != rust_tokens(&light, &file.new_text),
        "the old theme's tokens landed"
    );
    assert!(*tokens == rust_tokens(&shifted, &file.new_text));
    assert!(pipeline_stats(&view, cx).highlights > highlights);
    assert!(debug(&view, cx).styled_rows > 0);
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
    // The binary placeholder with its sizes (T2.5) from the background pass.
    assert_eq!(
        d.visible_rows[..2],
        ["== data.bin", "Binary file · 4 B → 4 B"]
    );
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

/// Wraps a provider whose blobs of files `from..` cannot be read while their
/// sizes are known: those files only ever get blob sizes, never counts.
struct SizesOnly {
    inner: Arc<MemProvider>,
    from: usize,
}

impl DiffProvider for SizesOnly {
    fn object_format(&self) -> ObjectFormat {
        self.inner.object_format()
    }

    fn files(&self) -> Arc<Vec<FileChange>> {
        self.inner.files()
    }

    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        let files = self.inner.files();
        let owner = files
            .iter()
            .position(|c| c.old_blob == *oid || c.new_blob == *oid);
        if owner.is_some_and(|f| f >= self.from) {
            anyhow::bail!("unreadable");
        }
        self.inner.load_blob(oid)
    }

    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        self.inner.blob_size(oid)
    }
}

/// `n` lines of exactly `width` bytes (the `\n` included).
fn lines_of(n: u32, width: usize) -> String {
    (0..n)
        .map(|i| format!("{:<w$}\n", format!("line {i}"), w = width - 1))
        .collect()
}

#[gpui_kit::test]
fn blob_sizes_refine_estimates_before_counts(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let mut specs = twenty_line_files(10);
    // 10: added, 50 lines in 3,200 bytes (100 lines at 32 bytes a line).
    specs.push(Spec::added("added.txt", &lines_of(50, 64)));
    // 11: modified, 64 bytes a side (4 rows at most).
    let small = lines_of(2, 32);
    specs.push(Spec::modified(
        "small.txt",
        &small,
        &small.replace("line", "LINE"),
    ));
    // 12: deleted, 1,280 bytes (40 lines).
    specs.push(Spec {
        old: Some(lines_of(20, 64)),
        new: None,
        ..Spec::added("deleted.txt", "")
    });
    // 13: a submodule has no blob sizes.
    specs.push(Spec {
        kind: FileKind::Submodule,
        ..Spec::modified("vendor/lib", "a", "b")
    });
    let provider = Arc::new(SizesOnly {
        inner: MemProvider::new(specs),
        from: 10,
    });
    let (view, cx) = open_idle(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let height =
        |cx: &mut VisualTestContext, f: u32| view.read_with(cx, |v, _| v.document().file_height(f));
    let unknown = HEADER_H + 20.0 * ROW_H;
    for f in 10..13 {
        assert_eq!(height(cx, f), unknown, "file {f}");
        assert_eq!(view.read_with(cx, |v, _| v.blob_sizes(f)), None);
    }
    let anchor = view.read_with(cx, |v, _| v.anchor());

    settle(cx);
    let sizes = |cx: &mut VisualTestContext, f: u32| view.read_with(cx, |v, _| v.blob_sizes(f));
    assert_eq!(sizes(cx, 10), Some((0, 3200)));
    assert_eq!(sizes(cx, 11), Some((64, 64)));
    assert_eq!(sizes(cx, 12), Some((1280, 0)));
    assert_eq!(sizes(cx, 13), None);
    // Without counts (their blobs cannot be read), the estimates come from
    // the sizes alone: 32 bytes a line.
    for f in 10..14 {
        assert_eq!(counts(&view, cx, f), None, "file {f}");
    }
    assert_eq!(height(cx, 10), HEADER_H + 100.0 * ROW_H);
    assert_eq!(height(cx, 11), HEADER_H + 4.0 * ROW_H);
    assert_eq!(height(cx, 12), HEADER_H + 40.0 * ROW_H);
    // Readable files' counts won over their sizes.
    assert_eq!(sizes(cx, 5), Some((0, 10 * 7 + 10 * 8)));
    assert!(counts(&view, cx, 5).is_some());
    assert_eq!(view.read_with(cx, |v, _| v.anchor()), anchor);
}

#[gpui_kit::test]
fn theme_change_rehighlights_only_the_window(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(rust_files(30));
    let mut opts = options(LayoutMode::Unified);
    opts.window_screens = 0.0;
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let top = view.read_with(cx, |v, _| v.document().visible(400.));
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(20), cx));
    settle(cx);
    let window = view.read_with(cx, |v, _| v.document().visible(400.));
    assert!(top.end <= window.start, "{top:?} {window:?}");
    // The files at the top are still resident, with tokens. (Slots are file
    // indices in the identity order.)
    for f in top.start..top.end {
        match state(&view, cx, f) {
            FileState::Materialized(file) => assert!(file.new_tokens.is_some()),
            other => panic!("file {f}: {other:?}"),
        }
    }

    let shifted = shifted_theme();
    let before = pipeline_stats(&view, cx).highlights;
    view.update(cx, |v, cx| {
        let mut o = v.options().clone();
        o.theme = shifted.clone();
        v.set_options(o, cx)
    });
    settle(cx);
    // Both sides of each file in the window, and nothing else.
    let after = pipeline_stats(&view, cx).highlights;
    assert_eq!(after - before, 2 * u64::from(window.end - window.start));
    for f in window.start..window.end {
        let FileState::Materialized(file) = state(&view, cx, f) else {
            panic!("file {f}")
        };
        let tokens = file.new_tokens.as_deref().expect("highlighted");
        assert!(*tokens == rust_tokens(&shifted, &file.new_text), "file {f}");
    }
    // Resident files outside the window dropped the old tokens and wait.
    for f in top.start..top.end {
        let FileState::Materialized(file) = state(&view, cx, f) else {
            panic!("file {f}")
        };
        assert!(file.old_tokens.is_none() && file.new_tokens.is_none());
    }

    // Scrolled back, they are highlighted with the new theme.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    settle(cx);
    let back = pipeline_stats(&view, cx).highlights;
    assert_eq!(back - after, 2 * u64::from(top.end - top.start));
    for f in top.start..top.end {
        let FileState::Materialized(file) = state(&view, cx, f) else {
            panic!("file {f}")
        };
        let tokens = file.new_tokens.as_deref().expect("highlighted");
        assert!(*tokens == rust_tokens(&shifted, &file.new_text), "file {f}");
    }
}

#[gpui_kit::test]
fn eviction_runs_after_tokens_arrive(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Identical files: each takes the same bytes once loaded.
    let provider = MemProvider::new(rust_files(20));
    let mut opts = options(LayoutMode::Unified);
    opts.window_screens = 0.0;
    // A viewport shorter than one file: exactly one file is visible.
    let (view, cx) = open(cx, provider, opts, 1000., 100.);
    assert_eq!(
        view.read_with(cx, |v, _| v.document().visible(100.)),
        SlotRange { start: 0, end: 1 }
    );
    let (with_tokens, tokens) = view.read_with(cx, |v, _| {
        let d = v.document();
        let FileState::Materialized(f) = d.state(0) else {
            panic!("file 0 is loaded")
        };
        let bytes = |t: &Option<Arc<Tokens>>| t.as_ref().map_or(0, |t| t.heap_bytes());
        (
            d.resident_bytes(),
            bytes(&f.old_tokens) + bytes(&f.new_tokens),
        )
    });
    assert!(tokens > 0);
    // Room for file 0 with its tokens and another file without them.
    set_options(&view, cx, |o| {
        o.eviction_budget_bytes = 2 * with_tokens - tokens
    });

    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(10), cx));
    redraw(cx);
    while !state(&view, cx, 10).is_materialized() {
        assert!(cx.executor().tick(), "file 10 loads");
    }
    // File 10 is in without tokens: within the budget, file 0 stays.
    let FileState::Materialized(file) = state(&view, cx, 10) else {
        unreachable!()
    };
    assert!(file.old_tokens.is_none() && file.new_tokens.is_none());
    assert!(state(&view, cx, 0).is_materialized());

    // Its tokens push the resident bytes over the budget: file 0 goes.
    settle(cx);
    let FileState::Materialized(file) = state(&view, cx, 10) else {
        panic!("file 10 stays")
    };
    assert!(file.new_tokens.is_some());
    assert!(matches!(state(&view, cx, 0), FileState::Evicted));
}

#[gpui_kit::test]
fn file_over_100k_lines_on_one_side_highlights_the_other(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // The old side has 100,001 lines, the new side 5: the cutoff is per side.
    let old = "1\n".repeat(100_001);
    let new = "x = 1\n".repeat(5);
    let provider = MemProvider::new(vec![Spec::modified("big.py", &old, &new)]);
    let mut opts = options(LayoutMode::Unified);
    // No "Load diff" placeholder for the 100k removed lines.
    opts.large_file_changed_lines = 200_000;
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let (events, _sub) = record_events(&view, cx);
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 0,
            },
            cx,
        )
    });
    settle(cx);
    let FileState::Materialized(file) = state(&view, cx, 0) else {
        panic!("loaded")
    };
    assert!(file.old_tokens.is_none(), "the old side is over the cutoff");
    assert!(file.new_tokens.is_some(), "the new side is not");
    assert!(view.read_with(cx, |v, _| v.syntax_skipped(0)));
    assert_eq!(pipeline_stats(&view, cx).highlights, 1);
    let d = debug(&view, cx);
    assert!(
        d.visible_rows.iter().any(|r| r.ends_with("+ x = 1")),
        "{d:?}"
    );
    assert!(d.styled_rows > 0);
    assert_eq!(last_stats(&events).unhighlighted_rows, 0);
}

#[gpui_kit::test]
fn threshold_change_reloads_without_recounting(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(40));
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    let reads: Vec<usize> = (0..40).map(|f| provider.loads_of(f)).collect();
    // Line counts do not depend on the "Load diff" threshold: only the files
    // in the window are read again (as placeholders now, so more of them fit
    // in it), nothing is counted again.
    set_options(&view, cx, |o| o.large_file_changed_lines = 10);
    let window = view.read_with(cx, |v, _| v.document().materialize_range(400., 2.0));
    assert!(window.end < 40, "{window:?}");
    for f in 0..40u32 {
        let again = provider.loads_of(f as usize) - reads[f as usize];
        assert_eq!(again, usize::from(window.contains(f)), "file {f}");
        assert_eq!(
            counts(&view, cx, f),
            Some(FileCounts {
                additions: 20,
                deletions: 0
            })
        );
    }
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[1], "Large diff · 20 changed lines");
}

#[gpui_kit::test]
fn counts_fill_nearest_the_viewport_first(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(60));
    let mut opts = options(LayoutMode::Unified);
    // The window is the visible file (40) alone.
    opts.window_screens = 0.0;
    let (view, cx) = open_idle_at(
        cx,
        provider.clone(),
        opts,
        1000.,
        400.,
        ScrollTarget::File(40),
    );
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
    settle(cx);
    let order = provider.load_order();
    let painted = order.iter().position(|&f| f == MARK).expect("first paint");
    assert_eq!(order[..painted], [40], "{order:?}");
    // After first paint, the other files are counted nearest the viewport
    // first (below before above at the same distance), not in file order.
    let mut nearest = Vec::new();
    for d in 1..=40 {
        if 40 + d < 60 {
            nearest.push(40 + d);
        }
        nearest.push(40 - d);
    }
    assert_eq!(order[painted + 1..], nearest[..], "{order:?}");
}

#[gpui_kit::test]
fn binary_file_is_not_counted_again_after_reload(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let mut specs = twenty_line_files(20);
    specs.push(Spec::modified("far.dat", "x\n\0\n", "y\n\0\n"));
    let provider = MemProvider::new(specs);
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.document().files()[20].kind),
        FileKind::Binary
    );
    assert_eq!(provider.loads_of(20), 1);
    // New diff options count every text file again, but not a file already
    // found binary.
    set_options(&view, cx, |o| o.diff.ignore_whitespace = true);
    assert_eq!(provider.loads_of(20), 1);
    assert_eq!(provider.loads_of(19), 2);
}

#[gpui_kit::test]
fn queued_load_builds_rows_of_the_layout_on_screen(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Nine modified files after an added one, whose rows are unified in
    // both layouts (one full-width pane).
    let mut specs = vec![Spec::added("new.txt", &numbered("line", 20).concat())];
    specs.extend((1..10).map(|i| {
        let (old, new) = with_changes(20, &[3]);
        Spec::modified(&format!("f{i:02}.txt"), &old, &new)
    }));
    let provider = MemProvider::new(specs);
    let (view, cx) = open_idle(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    // The first frame queued the window's loads for unified rows; the layout
    // changes before they run.
    assert!(matches!(state(&view, cx, 0), FileState::Loading { .. }));
    view.update(cx, |v, cx| {
        let mut o = v.options().clone();
        o.layout = LayoutMode::Split;
        v.set_options(o, cx)
    });
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.effective_layout()),
        Layout::Split
    );
    let window = view.read_with(cx, |v, _| v.document().materialize_range(400., 2.0));
    assert!(
        window.contains(0) && window.end - window.start > 2,
        "{window:?}"
    );
    for f in window.start..window.end {
        let FileState::Materialized(file) = state(&view, cx, f) else {
            panic!("file {f}")
        };
        let one_sided = f == 0;
        assert_eq!(file.rows_split.get().is_some(), !one_sided, "file {f}");
        assert_eq!(file.rows_unified.get().is_some(), one_sided, "file {f}");
    }
}

#[gpui_kit::test]
fn counts_of_window_files_repaint_as_they_arrive(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // A generated file is never loaded: its counts come from the background
    // pass, and it is on screen.
    let (old, new) = with_changes(40, &[2, 20]);
    let mut specs = vec![Spec::generated("package-lock.json", &old, &new)];
    specs.extend(twenty_line_files(40));
    let provider = MemProvider::new(specs);
    let (view, cx) = open_idle(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    // The generated file's counts as each frame saw them.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |view, e: &ViewportEvent, cx| {
            if let ViewportEvent::FrameStats(_) = e {
                sink.borrow_mut().push(view.read(cx).file_counts(0));
            }
        })
    });
    // Background work only: every frame from here on is one a result asked
    // for.
    cx.run_until_parked();
    let counted = Some(FileCounts {
        additions: 2,
        deletions: 2,
    });
    assert_eq!(counts(&view, cx, 0), counted);
    let seen = seen.borrow();
    assert_eq!(seen.first(), Some(&None), "{seen:?}");
    assert_eq!(seen.last(), Some(&counted), "{seen:?}");
}

#[gpui_kit::test]
fn counts_updated_is_emitted_once_per_batch(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let specs: Vec<Spec> = (0..50)
        .map(|i| {
            let (old, new) = with_changes(40, &[2, 20]);
            Spec::modified(&format!("src/f{i:02}.txt"), &old, &new)
        })
        .collect();
    let provider = MemProvider::new(specs);
    let (view, cx) = open_idle(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let updates = Rc::new(RefCell::new(0usize));
    let sink = updates.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &ViewportEvent, _| {
            if *e == ViewportEvent::CountsUpdated {
                *sink.borrow_mut() += 1;
            }
        })
    });
    let known = |cx: &mut VisualTestContext| {
        (0..50)
            .filter(|&f| view.read_with(cx, |v, _| v.file_counts(f)).is_some())
            .count()
    };
    // One task at a time: a task that lands counts (a load, or a chunk of
    // the background pass) emits exactly one event; any other emits none.
    let mut landed = 0;
    loop {
        let (k, n) = (known(cx), *updates.borrow());
        if !cx.executor().tick() {
            break;
        }
        let (k2, n2) = (known(cx), *updates.borrow());
        assert!(n2 - n <= 1, "{} events in one task", n2 - n);
        if k2 > k {
            assert_eq!(n2 - n, 1, "counts {k} → {k2} without an event");
            landed += 1;
        }
    }
    assert_eq!(known(cx), 50);
    // Far fewer events than files: the pass counts many files per batch.
    let total = *updates.borrow();
    assert!(
        total >= landed && total < 30,
        "{total} events, {landed} landings"
    );
    // Frames alone emit none.
    settle(cx);
    let before = *updates.borrow();
    redraw(cx);
    wheel(cx, 0.0);
    assert_eq!(*updates.borrow(), before);
}

/// Opens a 1000 × 400 unified viewport over `provider` with `hidden` files
/// hidden before its first frame, and lets every background task finish.
fn open_with_hidden<'a>(
    cx: &'a mut TestAppContext,
    provider: Arc<MemProvider>,
    hidden: &'static [u32],
) -> (Entity<DiffViewport>, &'a mut VisualTestContext) {
    assert_sandboxed();
    let window = cx.open_window(size(px(1000.), px(400.)), move |window, cx| {
        let mut view = DiffViewport::new(provider, options(LayoutMode::Unified), window, cx);
        view.set_hidden(hidden, true, cx);
        view
    });
    let view = window.root(cx).expect("window has a root view");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    settle(cx);
    (view, cx)
}

#[gpui_kit::test]
fn pipeline_never_loads_hidden_files_but_counts_them(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(10));
    let (view, cx) = open_with_hidden(cx, provider.clone(), &[1, 2]);
    // The window at the top ([0, 1200): 400 px and two screens below) holds
    // files 0 (0..445), 3 (445..890) and 4 (890..1335): hidden files have no
    // height, so they are not in its way.
    for f in [0, 3, 4] {
        assert!(state(&view, cx, f).is_materialized(), "file {f}");
    }
    for f in [1, 2, 5] {
        assert!(
            matches!(state(&view, cx, f), FileState::Estimated),
            "file {f}"
        );
    }
    assert_eq!(pipeline_stats(&view, cx).loads, 3);
    // The counts pass covers every file, hidden or not: 20 added lines each.
    for f in 0..10 {
        assert_eq!(
            counts(&view, cx, f),
            Some(FileCounts {
                additions: 20,
                deletions: 0
            }),
            "file {f}"
        );
    }
    // Hidden files and files outside the window were read once (one blob
    // per added file), by that pass alone.
    for f in [1, 2, 5, 6, 7, 8, 9] {
        assert_eq!(provider.loads_of(f), 1, "file {f}");
    }
    let rows = debug(&view, cx).visible_rows;
    assert!(
        !rows.iter().any(|r| r == "== f01.txt" || r == "== f02.txt"),
        "{rows:?}"
    );
}

#[gpui_kit::test]
fn hiding_a_loading_file_cancels_its_load(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(twenty_line_files(10));
    let (view, cx) = open_idle(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    // The first frame queued the window at the top: files 0, 1 and 2.
    assert!(matches!(state(&view, cx, 1), FileState::Loading { .. }));
    view.update(cx, |v, cx| v.set_hidden(&[1], true, cx));
    redraw(cx);
    assert!(matches!(state(&view, cx, 1), FileState::Estimated));
    assert_eq!(pipeline_stats(&view, cx).cancelled, 1);
    settle(cx);
    // File 3 moved into the window instead; file 1 is only counted.
    assert!(state(&view, cx, 3).is_materialized());
    assert!(matches!(state(&view, cx, 1), FileState::Estimated));
    assert_eq!(counts(&view, cx, 1).map(|c| c.additions), Some(20));
}
