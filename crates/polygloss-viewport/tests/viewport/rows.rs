//! The rows' look (T6.7, design §11.6, OQ-54): one-sided files as one
//! full-width pane, bars at each pane's left edge, tinted numbers and number
//! gutters, and the other indicator styles.
//!
//! Flat geometry at 7.8 px a column: a number column is 4 columns (31.2 px),
//! numbers right-aligned half a column before its edge; the bars' indicator
//! column is half a column (3.9 px). One number column: a 1-digit number at
//! 31.2 − 3.9 − 7.8 = 19.5, code at 35.1. Unified's two: the new number at
//! 62.4 − 3.9 − 7.8 = 50.7, code at 66.3.

use gpui_kit::TestAppContext;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{Indicators, LayoutMode};

use crate::support::*;

/// Ten numbered lines, line 4 changed: one hunk with a 1-line gap above and a
/// 2-line gap below.
fn one_change() -> Spec {
    let old = numbered("line", 10);
    let mut new = old.clone();
    new[4] = "LINE 4\n".to_owned();
    Spec::modified("src/a.rs", &old.concat(), &new.concat())
}

const ONE_NUMBER_X: f32 = 19.5;
const ONE_CODE_X: f32 = 35.1;
const NEW_NUMBER_X: f32 = 50.7;
const TWO_CODE_X: f32 = 66.3;

/// Texts painted in visible row `i` with their colors, left to right.
fn colors_in_row(d: &polygloss_viewport::ViewportDebug, i: usize) -> Vec<(String, gpui_kit::Hsla)> {
    let (top, height) = d.row_bounds[i];
    let mut out: Vec<(f32, String, gpui_kit::Hsla)> = d
        .painted_text
        .iter()
        .zip(&d.painted_text_colors)
        .filter(|((_, y, _), _)| *y >= top && *y < top + height)
        .map(|((x, _, t), c)| (*x, t.clone(), *c))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, t, c)| (t, c)).collect()
}

/// Asserts `actual` rectangles are `expected` (in order) within half a
/// point: GPUI snaps quads to device pixels.
fn assert_rects(actual: &[(f32, f32, f32, f32)], expected: &[(f32, f32, f32, f32)]) {
    let close = |a: &(f32, f32, f32, f32), e: &(f32, f32, f32, f32)| {
        [(a.0, e.0), (a.1, e.1), (a.2, e.2), (a.3, e.3)]
            .iter()
            .all(|(p, q)| (p - q).abs() <= 0.5)
    };
    let same =
        actual.len() == expected.len() && actual.iter().zip(expected).all(|(a, e)| close(a, e));
    assert!(same, "rects {actual:?}, expected {expected:?}");
}

#[gpui_kit::test]
fn added_file_renders_one_full_width_pane_in_split(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::added("src/new.rs", "alpha\nbeta\n")]);
    let opts = card_options(LayoutMode::Split);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(d.layout, Layout::Split);
    assert_eq!(
        d.visible_rows,
        vec![
            "== src/new.rs".to_owned(),
            unified(None, Some(1), '+', "alpha"),
            unified(None, Some(2), '+', "beta"),
        ]
    );
    // The card's inner width is 17..983: one number column, then the code.
    assert_texts(
        &texts_in_row(&d, 1),
        &[(17.0 + ONE_NUMBER_X, "1"), (17.0 + ONE_CODE_X, "alpha")],
    );
    assert_texts(
        &texts_in_row(&d, 2),
        &[(17.0 + ONE_NUMBER_X, "2"), (17.0 + ONE_CODE_X, "beta")],
    );
    // Each row's tint spans the whole inner width; no empty half, no
    // divider between halves.
    assert_rects(
        &quads_of(cx, theme.added_background),
        &[(17.0, 45.0, 966.0, 20.0), (17.0, 65.0, 966.0, 20.0)],
    );
    assert!(quads_of(cx, theme.empty_cell).is_empty());
    assert!(quads_of(cx, theme.border).is_empty());
}

#[gpui_kit::test]
fn deleted_file_renders_one_pane_with_old_numbers(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::deleted("old.rs", "x\ny\n")]);
    let opts = options(LayoutMode::Split);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        vec![
            "== old.rs".to_owned(),
            unified(Some(1), None, '-', "x"),
            unified(Some(2), None, '-', "y"),
        ]
    );
    assert_texts(
        &texts_in_row(&d, 1),
        &[(ONE_NUMBER_X, "1"), (ONE_CODE_X, "x")],
    );
    assert_eq!(
        colors_in_row(&d, 1)[0],
        ("1".to_owned(), theme.removed_line_number)
    );
    assert_rects(
        &quads_of(cx, theme.removed_background),
        &[(0.0, 45.0, 1000.0, 20.0), (0.0, 65.0, 1000.0, 20.0)],
    );
    assert!(quads_of(cx, theme.empty_cell).is_empty());
}

#[gpui_kit::test]
fn one_sided_file_in_unified_has_one_number_column(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", "alpha\n"),
        Spec::modified("b.rs", "a\n", "a\nX\n"),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows,
        vec![
            "== a.rs".to_owned(),
            unified(None, Some(1), '+', "alpha"),
            "== b.rs".to_owned(),
            unified(Some(1), Some(1), ' ', "a"),
            unified(None, Some(2), '+', "X"),
        ]
    );
    // The added file: one number column. The modified one: two.
    assert_texts(
        &texts_in_row(&d, 1),
        &[(ONE_NUMBER_X, "1"), (ONE_CODE_X, "alpha")],
    );
    assert_texts(
        &texts_in_row(&d, 3),
        &[(ONE_NUMBER_X, "1"), (NEW_NUMBER_X, "1"), (TWO_CODE_X, "a")],
    );
    assert_texts(
        &texts_in_row(&d, 4),
        &[(NEW_NUMBER_X, "2"), (TWO_CODE_X, "X")],
    );
}

#[gpui_kit::test]
fn layout_toggle_leaves_one_sided_files_alone(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::added("a.rs", "one\ntwo\nthree\n"),
        Spec::modified("b.rs", "a\nb\n", "a\nX\nb\n"),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1000., 600.);
    // The added file's rows and everything painted in them.
    let snapshot = |d: &polygloss_viewport::ViewportDebug| {
        let rows: Vec<_> = (0..4)
            .map(|i| {
                (
                    d.visible_rows[i].clone(),
                    texts_in_row(d, i),
                    d.row_bounds[i],
                )
            })
            .collect();
        let b = d.visible_rows[4..].to_vec();
        (rows, b)
    };
    let (split_a, split_b) = snapshot(&debug(&view, cx));
    set_options(&view, cx, |o| o.layout = LayoutMode::Unified);
    let d = debug(&view, cx);
    assert_eq!(d.layout, Layout::Unified);
    let (unified_a, unified_b) = snapshot(&d);
    assert_eq!(unified_a, split_a);
    assert_eq!(split_a[1].0, unified(None, Some(1), '+', "one"));
    // The modified file changes with the layout.
    assert_eq!(split_b[2], split(None, Some((2, '+', "X"))));
    assert_eq!(unified_b[2], unified(None, Some(2), '+', "X"));
}

#[gpui_kit::test]
fn bars_sit_at_each_panes_left_edge(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let opts = card_options(LayoutMode::Split);
    let theme = opts.theme.clone();
    assert_eq!(opts.style.indicators, Indicators::Bars, "bars by default");
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    // Header 0..45, the gap row 45..77, three context rows, then the changed
    // row at 137. The halves start at 17 and 17 + 483 = 500.
    assert_eq!(debug(&view, cx).row_bounds[5], (137.0, ROW_H));
    assert_rects(
        &quads_of(cx, theme.removed_accent),
        &[(17.0, 137.0, 3.0, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_accent),
        &[(500.0, 137.0, 3.0, 20.0)],
    );
    // Unified: both bars at the card's inner left edge, one row each.
    set_options(&view, cx, |o| o.layout = LayoutMode::Unified);
    assert_rects(
        &quads_of(cx, theme.removed_accent),
        &[(17.0, 137.0, 3.0, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_accent),
        &[(17.0, 157.0, 3.0, 20.0)],
    );
}

#[gpui_kit::test]
fn bars_draw_no_indicator_glyph(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[5], unified(Some(5), None, '-', "line 4"));
    // Numbers, then the code half a column after them: no `-`/`+`.
    assert_texts(
        &texts_in_row(&d, 5),
        &[(ONE_NUMBER_X, "5"), (TWO_CODE_X, "line 4")],
    );
    assert_texts(
        &texts_in_row(&d, 6),
        &[(NEW_NUMBER_X, "5"), (TWO_CODE_X, "LINE 4")],
    );
}

#[gpui_kit::test]
fn changed_rows_tint_numbers_and_gutter(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let opts = card_options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    let d = debug(&view, cx);
    let number = |i: usize| colors_in_row(&d, i)[0].clone();
    assert_eq!(number(4), ("4".to_owned(), theme.line_number));
    assert_eq!(number(5), ("5".to_owned(), theme.removed_line_number));
    assert_eq!(number(6), ("5".to_owned(), theme.added_line_number));
    // The number gutter (both columns, 17..79.4) of each changed row.
    assert_rects(
        &quads_of(cx, theme.removed_gutter),
        &[(17.0, 137.0, 62.4, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_gutter),
        &[(17.0, 157.0, 62.4, 20.0)],
    );
    // Split: each half's number column.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    assert_rects(
        &quads_of(cx, theme.removed_gutter),
        &[(17.0, 137.0, 31.2, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_gutter),
        &[(500.0, 137.0, 31.2, 20.0)],
    );
}

#[gpui_kit::test]
fn plus_minus_and_none_still_render(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let mut opts = options(LayoutMode::Unified);
    opts.style.indicators = Indicators::PlusMinus;
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    // `+-`: a 2-column indicator column (62.4..78) with the glyph half a
    // column in, the code after it; no bars.
    let d = debug(&view, cx);
    assert_texts(
        &texts_in_row(&d, 5),
        &[(ONE_NUMBER_X, "5"), (66.3, "-"), (78.0, "line 4")],
    );
    assert!(quads_of(cx, theme.removed_accent).is_empty());
    // `none`: no glyph, no bar, the code half a column after the numbers.
    set_options(&view, cx, |o| o.style.indicators = Indicators::None);
    let d = debug(&view, cx);
    assert_texts(
        &texts_in_row(&d, 5),
        &[(ONE_NUMBER_X, "5"), (TWO_CODE_X, "line 4")],
    );
    assert!(quads_of(cx, theme.removed_accent).is_empty());
    assert!(quads_of(cx, theme.added_accent).is_empty());
}
