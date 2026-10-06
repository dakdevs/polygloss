//! The rows' look (T6.7, design §11.6, OQ-54): one-sided files as one
//! full-width pane, bars at each pane's left edge, tinted numbers and number
//! gutters, and the other indicator styles.
//!
//! Geometry at 7.8 pt a digit (ADR-0031 C1), from the rows' left edge (the
//! viewport's in the flat layout): numbers have at least three digits; the
//! gutter is the 4 pt bar, 4 pt, the number cells and 8 pt, rounded up to
//! the 4 pt grid (at least 40); numbers are right-aligned 8 pt before its
//! end; code starts 10 pt after it. One column: a 40 pt gutter, a 1-digit
//! number at 32 − 7.8 = 24.2, code at 50. Unified's two (3 × 7.8 apart plus
//! 8): a 72 pt gutter, the new number at 64 − 7.8 = 56.2, the old one at
//! 64 − 31.4 − 7.8 = 24.8, code at 82.

use gpui_kit::{TestAppContext, px};
use polygloss_diff::Side;
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

/// A card's inner left edge in a viewport with the default cards: a 12 pt
/// margin and a 1 pt border.
const INNER: f32 = 13.0;

const ONE_NUMBER_X: f32 = 24.2;
const ONE_CODE_X: f32 = 50.0;
const OLD_NUMBER_X: f32 = 24.8;
const NEW_NUMBER_X: f32 = 56.2;
const TWO_CODE_X: f32 = 82.0;

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
    // The card's inner width is 13..987: one number column, then the code.
    assert_texts(
        &texts_in_row(&d, 1),
        &[(INNER + ONE_NUMBER_X, "1"), (INNER + ONE_CODE_X, "alpha")],
    );
    assert_texts(
        &texts_in_row(&d, 2),
        &[(INNER + ONE_NUMBER_X, "2"), (INNER + ONE_CODE_X, "beta")],
    );
    // Each row's tint spans the whole inner width; no empty half, no
    // divider between halves.
    let row = |i: f32| (INNER, HEADER_H + i * ROW_H, 974.0, ROW_H);
    assert_rects(&quads_of(cx, theme.added_background), &[row(0.0), row(1.0)]);
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
    let row = |i: f32| (0.0, HEADER_H + i * ROW_H, 1000.0, ROW_H);
    assert_rects(
        &quads_of(cx, theme.removed_background),
        &[row(0.0), row(1.0)],
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
        &[(OLD_NUMBER_X, "1"), (NEW_NUMBER_X, "1"), (TWO_CODE_X, "a")],
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
    // Header 0..46, the gap row 46..78, three context rows, then the changed
    // row at 138. The halves start at 13 and 13 + 487 = 500. Bars are 4 pt.
    assert_eq!(debug(&view, cx).row_bounds[5], (138.0, ROW_H));
    assert_rects(
        &quads_of(cx, theme.removed_accent),
        &[(INNER, 138.0, 4.0, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_accent),
        &[(500.0, 138.0, 4.0, 20.0)],
    );
    // Unified: both bars at the card's inner left edge, one row each.
    set_options(&view, cx, |o| o.layout = LayoutMode::Unified);
    assert_rects(
        &quads_of(cx, theme.removed_accent),
        &[(INNER, 138.0, 4.0, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_accent),
        &[(INNER, 158.0, 4.0, 20.0)],
    );
}

#[gpui_kit::test]
fn bars_draw_no_indicator_glyph(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[5], unified(Some(5), None, '-', "line 4"));
    // Numbers, then the code 10 pt after the gutter: no `-`/`+`.
    assert_texts(
        &texts_in_row(&d, 5),
        &[(OLD_NUMBER_X, "5"), (TWO_CODE_X, "line 4")],
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
    // The number gutter (both columns, 13..85) of each changed row.
    assert_rects(
        &quads_of(cx, theme.removed_gutter),
        &[(INNER, 138.0, 72.0, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_gutter),
        &[(INNER, 158.0, 72.0, 20.0)],
    );
    // Split: each half's one-column gutter.
    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    assert_rects(
        &quads_of(cx, theme.removed_gutter),
        &[(INNER, 138.0, 40.0, 20.0)],
    );
    assert_rects(
        &quads_of(cx, theme.added_gutter),
        &[(500.0, 138.0, 40.0, 20.0)],
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
    // `+-`: the indicator's 2-advance cell after the gutter (72..87.6)
    // with the glyph centered in it, the code after it; no bars.
    let d = debug(&view, cx);
    assert_texts(
        &texts_in_row(&d, 5),
        &[
            (OLD_NUMBER_X, "5"),
            (72.0 + 3.9, "-"),
            (72.0 + 15.6, "line 4"),
        ],
    );
    assert!(quads_of(cx, theme.removed_accent).is_empty());
    // `none`: no glyph, no bar, the code 10 pt after the gutter.
    set_options(&view, cx, |o| o.style.indicators = Indicators::None);
    let d = debug(&view, cx);
    assert_texts(
        &texts_in_row(&d, 5),
        &[(OLD_NUMBER_X, "5"), (TWO_CODE_X, "line 4")],
    );
    assert!(quads_of(cx, theme.removed_accent).is_empty());
    assert!(quads_of(cx, theme.added_accent).is_empty());
}

// ---------------------------------------------------------------------------
// the column grid (T7.4, ADR-0031 C1)
//
// Hand-computed at 7.8 pt a digit from a card's inner edge, 13 pt into the
// viewport (a 12 pt margin and a 1 pt border). The gutter holds the 4 pt
// change bar, 4 pt, the number cells (`digits · 7.8`; unified's two 8 pt
// apart) and 8 pt, rounded up to the 4 pt grid and at least 40 pt; each
// number is right-aligned in its cell, and code starts 10 pt after the
// gutter.

#[gpui_kit::test]
fn gutter_grows_with_digits(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // 1,200 lines, four digits: 4 + 4 + 4 × 7.8 + 8 = 47.2, so 48; code at
    // 48 + 10.
    let provider = MemProvider::new(vec![Spec::added("big.rs", &numbered("x", 1200).concat())]);
    let opts = card_options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[1], unified(None, Some(1), '+', "x 0"));
    let (top, _) = d.row_bounds[1];
    assert_rects(
        &quads_of(cx, theme.added_gutter)[..1],
        &[(INNER, top, 48.0, ROW_H)],
    );
    assert_texts(
        &texts_in_row(&d, 1),
        &[(INNER + 48.0 - 8.0 - ADVANCE, "1"), (INNER + 58.0, "x 0")],
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.code_x(0, Side::New)),
        Some(px(58.0))
    );
}

#[gpui_kit::test]
fn unified_gutter_holds_both_columns(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Three digits in two columns: 4 + 4 + 2 × 23.4 + 8 + 8 = 70.8, so 72;
    // code at 72 + 10.
    let provider = MemProvider::new(vec![one_change()]);
    let opts = card_options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 600.);
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[4], unified(Some(4), Some(4), ' ', "line 3"));
    let texts = texts_in_row(&d, 4);
    assert_eq!(
        texts.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
        ["4", "4", "line 3"]
    );
    // One-digit numbers: each right edge is its x plus one advance.
    let (old_right, new_right) = (texts[0].0 + ADVANCE, texts[1].0 + ADVANCE);
    assert!(
        near(new_right - old_right, 8.0 + 3.0 * ADVANCE),
        "{old_right} {new_right}"
    );
    assert!(near(texts[2].0, INNER + 82.0), "{texts:?}");
    let (top, _) = d.row_bounds[5];
    assert_rects(
        &quads_of(cx, theme.removed_gutter),
        &[(INNER, top, 72.0, ROW_H)],
    );
    assert_eq!(
        view.read_with(cx, |v, _| v.code_x(0, Side::Old)),
        Some(px(82.0))
    );
}

#[gpui_kit::test]
fn plus_minus_code_follows_the_indicator_cell(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // One column of three digits: a 40 pt gutter, then the indicator's cell
    // of two advances (15.6) instead of the 10 pt pad.
    let provider = MemProvider::new(vec![Spec::added("new.rs", "alpha\n")]);
    let mut opts = card_options(LayoutMode::Unified);
    opts.style.indicators = Indicators::PlusMinus;
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let d = debug(&view, cx);
    let code = texts_in_row(&d, 1)
        .into_iter()
        .find(|t| t.1 == "alpha")
        .expect("the code")
        .0;
    assert!((code - (INNER + 55.6)).abs() <= 0.1, "{code}");
}

#[gpui_kit::test]
fn split_halves_have_their_own_gutter(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // 1400 pt: the inner width (1374) splits at 13 + 687 = 700. Each half
    // has one 40 pt gutter from its own left edge and code 50 pt in.
    let provider = MemProvider::new(vec![one_change()]);
    let opts = card_options(LayoutMode::Split);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1400., 600.);
    let d = debug(&view, cx);
    assert_eq!(
        d.visible_rows[5],
        split(Some((5, '-', "line 4")), Some((5, '+', "LINE 4")))
    );
    let (top, _) = d.row_bounds[5];
    const HALF: f32 = 700.0;
    assert_rects(
        &quads_of(cx, theme.removed_gutter),
        &[(INNER, top, 40.0, ROW_H)],
    );
    assert_rects(
        &quads_of(cx, theme.added_gutter),
        &[(HALF, top, 40.0, ROW_H)],
    );
    assert_texts(
        &texts_in_row(&d, 5),
        &[
            (INNER + 32.0 - ADVANCE, "5"),
            (INNER + 50.0, "line 4"),
            (HALF + 32.0 - ADVANCE, "5"),
            (HALF + 50.0, "LINE 4"),
        ],
    );
    let code_x = |side| view.read_with(cx, |v, _| v.code_x(0, side));
    assert_eq!(code_x(Side::Old), Some(px(50.0)));
    assert_eq!(code_x(Side::New), Some(px(HALF - INNER + 50.0)));
}
