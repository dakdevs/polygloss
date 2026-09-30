//! Viewport view and element (T2.4, design §11.6, §12.4): painting split and
//! unified rows, the auto layout, scrolling, the shaped-line cache, word
//! highlights, diff styles and wrapping.

use std::sync::Arc;

use gpui_kit::TestAppContext;
use polygloss_diff::Side;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{
    DiffStyle, DiffViewport, FileState, Indicators, LayoutMode, RowKey, ScrollAnchor, ScrollTarget,
    ViewportEvent,
};

use crate::support::*;

/// `old` with line `i` replaced by `with`.
fn replace(lines: &[String], i: usize, with: &str) -> String {
    let mut v = lines.to_vec();
    v[i] = format!("{with}\n");
    v.concat()
}

/// Ten numbered lines, line 4 changed: one hunk with a 1-line gap above and a
/// 2-line gap below.
fn one_change() -> Spec {
    let old = numbered("line", 10);
    Spec::modified("src/a.rs", &old.concat(), &replace(&old, 4, "LINE 4"))
}

#[gpui_kit::test]
fn viewport_renders_first_rows_of_synthetic_diff(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change(), Spec::added("src/b.rs", "fn b() {}\n")]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let d = debug(&view, cx);
    let expected = vec![
        "== src/a.rs".to_owned(),
        "⋯ 1 unchanged line".to_owned(),
        unified(Some(2), Some(2), ' ', "line 1"),
        unified(Some(3), Some(3), ' ', "line 2"),
        unified(Some(4), Some(4), ' ', "line 3"),
        unified(Some(5), None, '-', "line 4"),
        unified(None, Some(5), '+', "LINE 4"),
        unified(Some(6), Some(6), ' ', "line 5"),
        unified(Some(7), Some(7), ' ', "line 6"),
        unified(Some(8), Some(8), ' ', "line 7"),
        "⋯ 2 unchanged lines".to_owned(),
        "== src/b.rs".to_owned(),
        unified(None, Some(1), '+', "fn b() {}"),
    ];
    assert_eq!(d.visible_rows, expected);
    assert_eq!(d.layout, Layout::Unified);
    assert_eq!(d.anchor, ScrollAnchor::default());
    // Rows are stacked without gaps: header 40, gap row 32, code rows 20.
    assert_eq!(d.row_bounds[0], (0.0, HEADER_H));
    assert_eq!(d.row_bounds[1], (HEADER_H, 32.0));
    assert_eq!(d.row_bounds[2], (HEADER_H + 32.0, ROW_H));
}

#[gpui_kit::test]
fn viewport_unified_has_two_line_number_columns(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // An insertion above shifts the new numbering: context rows show both.
    let provider = MemProvider::new(vec![Spec::modified(
        "src/a.rs",
        "a\nb\nc\n",
        "a\nX\nb\nc\n",
    )]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        vec![
            "== src/a.rs".to_owned(),
            unified(Some(1), Some(1), ' ', "a"),
            unified(None, Some(2), '+', "X"),
            unified(Some(2), Some(3), ' ', "b"),
            unified(Some(3), Some(4), ' ', "c"),
        ]
    );
    // What the gutter painted, and where: two number columns of 4 advances
    // (3 digits + 1) with numbers right-aligned half an advance from their
    // edge, then the 2-advance indicator column, then the code.
    let old_x = 4.0 * ADVANCE - 0.5 * ADVANCE - ADVANCE;
    let new_x = 8.0 * ADVANCE - 0.5 * ADVANCE - ADVANCE;
    let (marker_x, code_x) = (8.5 * ADVANCE, 10.0 * ADVANCE);
    assert_texts(
        &texts_in_row(&d, 1),
        &[(old_x, "1"), (new_x, "1"), (code_x, "a")],
    );
    assert_texts(
        &texts_in_row(&d, 2),
        &[(new_x, "2"), (marker_x, "+"), (code_x, "X")],
    );
    assert_texts(
        &texts_in_row(&d, 3),
        &[(old_x, "2"), (new_x, "3"), (code_x, "b")],
    );
}

#[gpui_kit::test]
fn viewport_split_left_old_right_new(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        one_change(),
        Spec::modified("src/c.rs", "a\nb\n", "a\nX\nb\n"),
    ]);
    let opts = options(LayoutMode::Split);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    let d = debug(&view, cx);
    assert_eq!(d.layout, Layout::Split);
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
            ctx(6, "line 5"),
            ctx(7, "line 6"),
            ctx(8, "line 7"),
            "⋯ 2 unchanged lines".to_owned(),
            "== src/c.rs".to_owned(),
            ctx(1, "a"),
            split(None, Some((2, '+', "X"))),
            split(Some((2, ' ', "b")), Some((3, ' ', "b"))),
        ]
    );
    // Old text is painted in the left half and new text in the right one
    // (which starts at 500): per half a 4-advance number column, a 2-advance
    // indicator column, then the code.
    let (num_x, marker_x, code_x) = (2.5 * ADVANCE, 4.5 * ADVANCE, 6.0 * ADVANCE);
    assert_texts(
        &texts_in_row(&d, 5),
        &[
            (num_x, "5"),
            (marker_x, "-"),
            (code_x, "line 4"),
            (500.0 + num_x, "5"),
            (500.0 + marker_x, "+"),
            (500.0 + code_x, "LINE 4"),
        ],
    );
    assert_texts(
        &texts_in_row(&d, 12),
        &[
            (500.0 + num_x, "2"),
            (500.0 + marker_x, "+"),
            (500.0 + code_x, "X"),
        ],
    );
    assert_texts(
        &texts_in_row(&d, 13),
        &[
            (num_x, "2"),
            (code_x, "b"),
            (500.0 + num_x, "3"),
            (500.0 + code_x, "b"),
        ],
    );
    // Removed backgrounds sit in the left half, added ones in the right half.
    let removed = quads_of(cx, theme.removed_background);
    let added = quads_of(cx, theme.added_background);
    assert!(!removed.is_empty() && !added.is_empty());
    assert!(removed.iter().all(|q| q.0 + q.2 <= 500.5), "{removed:?}");
    assert!(added.iter().all(|q| q.0 >= 499.5), "{added:?}");
    // A divider between the halves on every code row, not hidden under the
    // right half's backgrounds.
    let dividers = quads_of(cx, theme.border);
    assert!(
        dividers
            .iter()
            .any(|q| (q.0 - 499.0).abs() < 0.01 && q.2 <= 1.0),
        "{dividers:?}"
    );
}

#[gpui_kit::test]
fn viewport_auto_layout_switches_at_160_columns_with_hysteresis(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // 160 columns of 7.8 px = 1248 px; switching back needs ±8 columns.
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Auto), 1300., 400.);
    let layout_at = |cx: &mut gpui_kit::VisualTestContext, width: f32| {
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(400.)));
        settle(cx);
        view.read_with(cx, |v, _| v.effective_layout())
    };
    assert_eq!(
        view.read_with(cx, |v, _| v.effective_layout()),
        Layout::Split
    ); // 166.7 cols
    assert_eq!(layout_at(cx, 1200.), Layout::Split); // 153.8 >= 152
    assert_eq!(layout_at(cx, 1180.), Layout::Unified); // 151.3 < 152
    assert_eq!(layout_at(cx, 1300.), Layout::Unified); // 166.7 < 168
    assert_eq!(layout_at(cx, 1320.), Layout::Split); // 169.2 >= 168
    // What is painted follows the effective layout.
    assert_eq!(debug(&view, cx).layout, Layout::Split);
    assert_eq!(layout_at(cx, 900.), Layout::Unified);
    assert_eq!(debug(&view, cx).layout, Layout::Unified);
}

#[gpui_kit::test]
fn viewport_auto_layout_starts_unified_below_160_columns(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Auto), 1240., 400.); // 159 cols
    assert_eq!(
        view.read_with(cx, |v, _| v.effective_layout()),
        Layout::Unified
    );
}

#[gpui_kit::test]
fn viewport_scroll_updates_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let a = numbered("a", 30).concat();
    let b = numbered("b", 20).concat();
    let provider = MemProvider::new(vec![Spec::added("a.rs", &a), Spec::added("b.rs", &b)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let (events, _sub) = record_events(&view, cx);

    // Header 40 + 13 rows of 20 = 300.
    wheel(cx, 300.);
    let anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(
        anchor,
        ScrollAnchor {
            file_idx: 0,
            row: RowKey::Line {
                side: Side::New,
                line: 13
            },
            offset_px: 0.0
        }
    );
    // a.rs's header stays pinned over its rows (T2.5).
    let rows = debug(&view, cx).visible_rows;
    assert_eq!(rows[0], "== a.rs");
    assert_eq!(rows[1], unified(None, Some(14), '+', "a 13"));

    // Past the end: clamped to 1080 - 400 = 680, the top of b.rs's body.
    wheel(cx, 400.);
    let anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(
        anchor,
        ScrollAnchor {
            file_idx: 1,
            row: RowKey::Line {
                side: Side::New,
                line: 0
            },
            offset_px: 0.0
        }
    );
    let visible_changes: Vec<u32> = events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            ViewportEvent::VisibleFileChanged(f) => Some(*f),
            _ => None,
        })
        .collect();
    assert_eq!(visible_changes, vec![1]);

    // Back up into a.rs's last row (580..600 of its body): the anchor keeps
    // the offset within the row.
    wheel(cx, -50.);
    let anchor = view.read_with(cx, |v, _| v.anchor());
    assert_eq!(anchor.file_idx, 0);
    assert_eq!(
        anchor.row,
        RowKey::Line {
            side: Side::New,
            line: 29
        }
    );
    assert_eq!(anchor.offset_px, 10.0);
}

#[gpui_kit::test]
fn viewport_scroll_to_line_puts_it_at_top(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let a = numbered("a", 30).concat();
    let b = numbered("b", 60).concat();
    let provider = MemProvider::new(vec![Spec::added("a.rs", &a), Spec::added("b.rs", &b)]);
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
    let d = debug(&view, cx);
    assert_eq!(
        d.anchor.row,
        RowKey::Line {
            side: Side::New,
            line: 10
        }
    );
    assert_eq!(d.anchor.file_idx, 1);
    // At the top of what is visible: right below b.rs's header, pinned at
    // the viewport's top edge (T2.5), not under it.
    assert_eq!(d.visible_rows[0], "== b.rs");
    assert_eq!(d.row_bounds[0], (0.0, HEADER_H));
    let line = d
        .visible_rows
        .iter()
        .position(|r| *r == unified(None, Some(11), '+', "b 10"))
        .expect("line 10 is painted");
    assert_eq!(d.row_bounds[line], (HEADER_H, ROW_H));

    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    settle(cx);
    assert_eq!(debug(&view, cx).visible_rows[0], "== a.rs");
}

#[gpui_kit::test]
fn viewport_plain_text_then_tokens_same_geometry(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let src = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    let provider = MemProvider::new(vec![Spec::added("src/main.rs", src)]);
    let mut opts = options(LayoutMode::Split);
    opts.syntax = false;
    let (view, cx) = open(cx, provider, opts, 1300., 400.);
    let plain = debug(&view, cx);
    assert_eq!(plain.styled_rows, 0);
    assert_eq!(plain.visible_rows.len(), 5);

    set_options(&view, cx, |o| o.syntax = true);
    let styled = debug(&view, cx);
    assert!(styled.styled_rows >= 4, "{}", styled.styled_rows);
    assert_eq!(styled.visible_rows, plain.visible_rows);
    assert_eq!(styled.row_bounds, plain.row_bounds);
    assert_eq!(styled.anchor, plain.anchor);
}

#[gpui_kit::test]
fn shaped_line_cache_hits_on_rescroll(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let src = numbered("row", 400).concat();
    let provider = MemProvider::new(vec![Spec::added("a.txt", &src)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let first = debug(&view, cx);
    assert!(first.shaped_cache_misses > 0);
    // Only visible rows are shaped: per row a number, a marker and the code.
    let rows = first.visible_rows.len() as u64;
    assert!(rows < 30, "{rows}");
    assert!(first.shaped_cache_misses <= 3 * rows + 4, "{first:?}");

    wheel(cx, 400.);
    let down = debug(&view, cx);
    assert!(down.shaped_cache_misses > first.shaped_cache_misses);
    // Under the pinned header (T2.5).
    assert_eq!(down.visible_rows[1], unified(None, Some(19), '+', "row 18"));

    wheel(cx, -400.);
    let back = debug(&view, cx);
    assert_eq!(back.visible_rows, first.visible_rows);
    assert_eq!(
        back.shaped_cache_misses, down.shaped_cache_misses,
        "rows seen before are not shaped again"
    );
    assert!(back.shaped_cache_hits >= down.shaped_cache_hits + back.visible_rows.len() as u64);
}

#[gpui_kit::test]
fn word_ranges_painted_on_paired_lines(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::modified(
        "src/a.rs",
        "let value = 1;\n",
        "let value = 2;\nlet extra = 3;\n",
    )]);
    let opts = options(LayoutMode::Split);
    let theme = opts.theme.clone();
    let (_view, cx) = open(cx, provider, opts, 1300., 400.);
    // Only the paired line gets highlights: one char on each side.
    let removed = quads_of(cx, theme.removed_word);
    let added = quads_of(cx, theme.added_word);
    assert_eq!(removed.len(), 1, "{removed:?}");
    assert_eq!(added.len(), 1, "{added:?}");
    // GPUI snaps quad edges to device pixels (half a point at 2x).
    let snap = 0.5 + 1e-3;
    assert!((removed[0].2 - ADVANCE).abs() <= snap, "{removed:?}");
    assert!((added[0].2 - ADVANCE).abs() <= snap, "{added:?}");
    // Both highlights sit at the same offset within their half: the right
    // half starts 650 px to the right.
    assert!(
        (added[0].0 - removed[0].0 - 650.).abs() <= snap,
        "{removed:?} {added:?}"
    );
}

#[gpui_kit::test]
fn word_diff_off_paints_no_highlights(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::modified("a", "x = 1\n", "x = 2\n")]);
    let mut opts = options(LayoutMode::Split);
    opts.word_diff = None;
    let theme = opts.theme.clone();
    let (_view, cx) = open(cx, provider, opts, 1300., 400.);
    assert!(quads_of(cx, theme.removed_word).is_empty());
    assert!(!quads_of(cx, theme.removed_background).is_empty());
}

#[gpui_kit::test]
fn diff_style_bars_and_no_backgrounds(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    // Default: row backgrounds and `+`/`-` glyphs, no bars.
    assert!(!quads_of(cx, theme.removed_background).is_empty());
    assert!(!quads_of(cx, theme.added_background).is_empty());
    assert!(quads_of(cx, theme.removed_accent).is_empty());
    assert!(quads_of(cx, theme.added_accent).is_empty());

    set_options(&view, cx, |o| {
        o.style = DiffStyle {
            backgrounds: false,
            indicators: Indicators::Bars,
            wrap: false,
        }
    });
    assert!(quads_of(cx, theme.removed_background).is_empty());
    assert!(quads_of(cx, theme.added_background).is_empty());
    let bars = [
        quads_of(cx, theme.removed_accent),
        quads_of(cx, theme.added_accent),
    ];
    for bar in &bars {
        assert_eq!(bar.len(), 1, "{bars:?}");
        assert!(
            bar[0].2 <= 4.0 && (bar[0].3 - ROW_H).abs() < 0.01,
            "{bars:?}"
        );
    }
    // Word highlights do not depend on backgrounds.
    assert_eq!(quads_of(cx, theme.removed_word).len(), 1);

    set_options(&view, cx, |o| o.style.indicators = Indicators::None);
    assert!(quads_of(cx, theme.removed_accent).is_empty());
    assert!(quads_of(cx, theme.added_accent).is_empty());
}

#[gpui_kit::test]
fn wrap_on_wraps_long_lines_and_keeps_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let long = "word ".repeat(80); // 400 chars
    let tail: Vec<String> = numbered("tail", 40);
    let old = format!("a\nshort\nc\n{}", tail.concat());
    let new_tail = replace(&tail, 30, "TAIL 30");
    let new = format!("a\n{long}\n{long}\nc\n{new_tail}");
    let provider = MemProvider::new(vec![Spec::modified("src/w.rs", &old, &new)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1300., 600.);

    // Anchor below the long rows: new line 34 is "TAIL 30".
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 34,
            },
            cx,
        )
    });
    settle(cx);
    let before = debug(&view, cx);
    let row_h = |cx: &mut gpui_kit::VisualTestContext, line: u32| {
        view.read_with(cx, |v, _| {
            let layout = v.document().file_layout(0).unwrap();
            let row = layout
                .find(RowKey::Line {
                    side: Side::New,
                    line,
                })
                .unwrap();
            layout.row_height(row)
        })
    };
    assert_eq!(row_h(cx, 1), ROW_H);

    set_options(&view, cx, |o| o.style.wrap = true);
    let after = debug(&view, cx);
    assert_eq!(after.anchor, before.anchor);
    assert_eq!(after.visible_rows[0], before.visible_rows[0]);
    assert_eq!(after.row_bounds[0], before.row_bounds[0]);
    // Row 1 pairs "short" with a long line, row 2 has the long line alone: the
    // pair is as tall as its taller side, and that side spans several rows.
    let (pair, alone) = (row_h(cx, 1), row_h(cx, 2));
    assert_eq!(pair, alone);
    assert!(pair >= 2.0 * ROW_H, "{pair}");
    assert_eq!(pair % ROW_H, 0.0);

    // Once painted (measured, not estimated) the rule still holds.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    settle(cx);
    let (pair, alone) = (row_h(cx, 1), row_h(cx, 2));
    assert_eq!(pair, alone);
    assert!(pair >= 2.0 * ROW_H, "{pair}");
    let top = debug(&view, cx);
    assert_eq!(top.row_bounds[3].1, pair, "{:?}", top.row_bounds);

    // Unified wraps too, and turning wrap off restores single rows.
    set_options(&view, cx, |o| o.layout = LayoutMode::Unified);
    assert!(row_h(cx, 1) >= 2.0 * ROW_H);
    set_options(&view, cx, |o| o.style.wrap = false);
    assert_eq!(row_h(cx, 1), ROW_H);
}

#[gpui_kit::test]
fn special_files_render_without_loading_blobs(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::binary("img.png"),
        Spec::modified("a.rs", "a\n", "b\n"),
    ]);
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[0], "== img.png");
    // Sizes from `blob_size` (T2.5), not from reading the blobs.
    assert_eq!(d.visible_rows[1], "Binary file · 4 B → 4 B");
    assert_eq!(d.visible_rows[2], "== a.rs");
    // Only the text file's two blobs were read.
    assert_eq!(provider.load_count(), 2);
}

#[gpui_kit::test]
fn display_text_expands_tabs_and_hides_carriage_returns(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::added("a.go", "\tx := 1\r\nab\tc\n\u{1b}[0m\n")]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows[1..],
        [
            unified(None, Some(1), '+', "    x := 1"),
            unified(None, Some(2), '+', "ab  c"),
            unified(None, Some(3), '+', "␛[0m"),
        ]
    );
}

#[gpui_kit::test]
fn long_lines_are_cut_when_not_wrapping(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let huge = "x".repeat(200_000);
    let provider = MemProvider::new(vec![Spec::added("min.js", &format!("{huge}\n"))]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let d = debug(&view, cx);
    let text = d.visible_rows[1].rsplit(' ').next().unwrap();
    assert!(text.ends_with('…'), "{}", &text[text.len() - 10..]);
    assert!(text.chars().count() < 5_000, "{}", text.chars().count());
}

#[gpui_kit::test]
fn frame_stats_are_emitted_per_frame(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let (events, _sub) = record_events(&view, cx);
    wheel(cx, 20.);
    let stats: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            ViewportEvent::FrameStats(s) => Some(*s),
            _ => None,
        })
        .collect();
    assert!(!stats.is_empty());
    assert!(stats.iter().all(|s| s.visible_rows > 0));
}

#[gpui_kit::test]
fn theme_change_repaints_with_new_colors(cx: &mut TestAppContext) {
    let _sb = sandbox();
    use polygloss_highlight::{Appearance, pierre_theme};
    use polygloss_viewport::ViewportTheme;
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let dark = Arc::new(ViewportTheme::from_zed(pierre_theme(Appearance::Dark)));
    set_options(&view, cx, |o| o.theme = dark.clone());
    assert!(!quads_of(cx, dark.background).is_empty());
    assert!(!quads_of(cx, dark.removed_background).is_empty());
}

#[gpui_kit::test]
fn frame_stats_count_loading_and_unhighlighted_rows(cx: &mut TestAppContext) {
    use gpui_kit::{VisualTestContext, px, size};
    let _sb = sandbox();
    let src = "fn main() {\n    let x = 1;\n}\n";
    let provider = MemProvider::new(vec![Spec::added("src/main.rs", src)]);
    let mut opts = options(LayoutMode::Unified);
    opts.syntax = false;
    let window = cx.open_window(size(px(1000.), px(400.)), move |window, cx| {
        polygloss_viewport::DiffViewport::new(provider, opts, window, cx)
    });
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    let (events, _sub) = record_events(&view, cx);

    // Nothing has loaded yet: the body is a "Loading…" row.
    redraw(cx);
    assert_eq!(
        debug(&view, cx).visible_rows,
        ["== src/main.rs", "Loading…"]
    );
    let stats = last_stats(&events);
    assert_eq!(stats.loading_rows, 1);

    // Loaded, syntax off: every row painted, nothing pending.
    settle(cx);
    let stats = last_stats(&events);
    assert_eq!((stats.loading_rows, stats.unhighlighted_rows), (0, 0));

    // Syntax on: rows stay plain until the tokens arrive.
    view.update(cx, |v, cx| {
        let mut o = v.options().clone();
        o.syntax = true;
        v.set_options(o, cx)
    });
    redraw(cx);
    assert_eq!(last_stats(&events).unhighlighted_rows, 3);
    settle(cx);
    let stats = last_stats(&events);
    assert_eq!((stats.loading_rows, stats.unhighlighted_rows), (0, 0));
    assert_eq!(debug(&view, cx).styled_rows, 3);
}

/// The error label a file whose blobs could not be read shows.
fn assert_load_error(row: &str) {
    assert!(
        row.starts_with("Could not load this file: ") && row.contains("is missing"),
        "{row}"
    );
}

#[gpui_kit::test]
fn failed_load_shows_error_and_is_not_counted_as_loading(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change(), Spec::added("src/b.rs", "fn b() {}\n")]);
    provider.set_broken(0, true);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let (events, _sub) = record_events(&view, cx);
    redraw(cx);

    let d = debug(&view, cx);
    assert_eq!(d.visible_rows.len(), 4, "{:?}", d.visible_rows);
    assert_eq!(d.visible_rows[0], "== src/a.rs");
    assert_load_error(&d.visible_rows[1]);
    assert_eq!(
        d.visible_rows[2..],
        [
            "== src/b.rs".to_owned(),
            unified(None, Some(1), '+', "fn b() {}")
        ]
    );
    // The error is a one-label body of exact height, not an estimate.
    assert_eq!(d.row_bounds[1], (HEADER_H, PLACEHOLDER_H));
    assert_eq!(d.row_bounds[2].0, HEADER_H + PLACEHOLDER_H);
    assert!(view.read_with(cx, |v, _| v.document().is_exact(0)));
    // Nothing is loading any more: first paint is over.
    let stats = last_stats(&events);
    assert_eq!((stats.loading_rows, stats.unhighlighted_rows), (0, 0));
}

#[gpui_kit::test]
fn failed_reload_replaces_stale_rows_with_error(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        600.,
    );
    let loaded = debug(&view, cx).visible_rows;
    assert_eq!(loaded.len(), 11, "{loaded:?}");
    let (events, _sub) = record_events(&view, cx);

    // A diff option change reloads the file, and the reload fails: the old
    // rows give way to the error.
    provider.set_broken(0, true);
    set_options(&view, cx, |o| o.diff.ignore_whitespace = true);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows.len(), 2, "{:?}", d.visible_rows);
    assert_load_error(&d.visible_rows[1]);
    assert_eq!(d.row_bounds[1], (HEADER_H, PLACEHOLDER_H));
    let stats = last_stats(&events);
    assert_eq!((stats.loading_rows, stats.unhighlighted_rows), (0, 0));

    // The next reload retries: "Loading…" (counted) instead of the stale
    // error until the rows are back.
    provider.set_broken(0, false);
    view.update(cx, |v: &mut DiffViewport, cx| {
        let mut o = v.options().clone();
        o.diff.ignore_whitespace = false;
        v.set_options(o, cx)
    });
    redraw(cx);
    assert_eq!(debug(&view, cx).visible_rows, ["== src/a.rs", "Loading…"]);
    assert_eq!(last_stats(&events).loading_rows, 1);
    settle(cx);
    assert_eq!(debug(&view, cx).visible_rows, loaded);
    let stats = last_stats(&events);
    assert_eq!((stats.loading_rows, stats.unhighlighted_rows), (0, 0));
}

#[gpui_kit::test]
fn large_file_placeholder_keeps_no_rows(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let old = numbered("line", 10);
    let new = numbered("LINE", 10);
    let provider = MemProvider::new(vec![Spec::modified("big.rs", &old.concat(), &new.concat())]);
    let mut opts = options(LayoutMode::Unified);
    opts.large_file_changed_lines = 5;
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    assert_eq!(
        debug(&view, cx).visible_rows,
        ["== big.rs", "Large diff · 20 changed lines"]
    );
    // Only the placeholder shows: no rows were built for either layout.
    let built = view.read_with(cx, |v, _| match v.document().state(0) {
        FileState::Materialized(f) => {
            (f.rows_split.get().is_some(), f.rows_unified.get().is_some())
        }
        other => panic!("{other:?}"),
    });
    assert_eq!(built, (false, false));
}

#[gpui_kit::test]
fn frame_stats_shaped_lines_add_up_across_wrap_passes(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Ten 40-char words per line: at 117 columns wrapping at word boundaries
    // needs 5 rows where the estimate says 4, so the first wrapped frame is
    // measured and built again.
    let line = format!("{} ", "w".repeat(39)).repeat(10);
    let src: String = (0..20).map(|_| format!("{line}\n")).collect();
    let provider = MemProvider::new(vec![Spec::added("a.txt", &src)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let before = debug(&view, cx).shaped_cache_misses;
    let (events, _sub) = record_events(&view, cx);

    set_options(&view, cx, |o| o.style.wrap = true);
    let row_h = view.read_with(cx, |v, _| {
        v.document().file_layout(0).unwrap().row_height(0)
    });
    assert_eq!(row_h, 5.0 * ROW_H, "the row was measured");
    let shaped: u64 = all_stats(&events)
        .iter()
        .map(|s| u64::from(s.shaped_lines))
        .sum();
    let misses = debug(&view, cx).shaped_cache_misses - before;
    assert!(misses > 0);
    assert_eq!(shaped, misses, "every line shaped is reported once");
}

#[gpui_kit::test]
fn split_no_newline_markers_of_both_sides_share_one_row(cx: &mut TestAppContext) {
    let _sb = sandbox();
    const MARKER: &str = "\\ No newline at end of file";
    let provider = MemProvider::new(vec![Spec::modified("a.txt", "a\nb", "a\nc")]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1400., 400.);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        [
            "== a.txt".to_owned(),
            split(Some((1, ' ', "a")), Some((1, ' ', "a"))),
            split(Some((2, '-', "b")), Some((2, '+', "c"))),
            format!("{MARKER} │ {MARKER}"),
        ]
    );
    assert_eq!(d.row_bounds[3], (HEADER_H + 2.0 * ROW_H, ROW_H));
    // Both halves paint the marker on that row, left one in the left half.
    let markers: Vec<_> = d
        .painted_text
        .iter()
        .filter(|(_, _, t)| t == MARKER)
        .collect();
    assert_eq!(markers.len(), 2, "{:?}", d.painted_text);
    assert_eq!(markers[0].1, markers[1].1);
    assert!(markers[0].0 < 700.0 && markers[1].0 > 700.0, "{markers:?}");

    // Unified keeps one marker per side, each after its own line.
    view.update(cx, |v, cx| v.set_options(options(LayoutMode::Unified), cx));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows[2..],
        [
            unified(Some(2), None, '-', "b"),
            MARKER.to_owned(),
            unified(None, Some(2), '+', "c"),
            MARKER.to_owned(),
        ]
    );
}

#[gpui_kit::test]
fn code_font_draws_without_ligatures_unless_asked(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::modified(
        "src/a.rs",
        "fn a() -> u8 { 1 }\n",
        "fn a() -> u16 { 1 }\n",
    )]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    // Off by default: `->` stays two characters.
    let font = view.read_with(cx, |v, _| v.code_font().clone());
    assert!(!view.read_with(cx, |v, _| v.options().ligatures));
    assert_eq!(font.features, polygloss_viewport::code_font_features(false));
    assert_eq!(font.features.is_calt_enabled(), Some(false));
    assert!(
        font.features
            .tag_value_list()
            .contains(&("liga".to_owned(), 0))
    );
    // Turning them on re-resolves the font and shapes the lines again.
    let misses = debug(&view, cx).shaped_cache_misses;
    set_options(&view, cx, |o| o.ligatures = true);
    let font = view.read_with(cx, |v, _| v.code_font().clone());
    assert!(font.features.tag_value_list().is_empty());
    assert_eq!(font, polygloss_viewport::code_font("Lilex", true));
    assert!(debug(&view, cx).shaped_cache_misses > misses);
}
