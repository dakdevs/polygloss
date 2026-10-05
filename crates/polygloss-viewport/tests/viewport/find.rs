//! Find highlights (T5.10, design §11.14): the host's matcher marks every
//! match in the visible code, split and unified, the current match in a
//! stronger color; clearing the highlights removes the marks.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use polygloss_diff::Side;
use polygloss_viewport::{
    DiffViewport, FindCurrent, FindHighlights, FindMatcher, LayoutMode, ViewportTheme,
};

use crate::support::*;

/// Every case-insensitive `needle` in a line.
fn matcher(needle: &'static str) -> FindMatcher {
    Arc::new(move |line: &[u8]| {
        let hay = line.to_ascii_lowercase();
        let n = needle.as_bytes();
        let mut out: Vec<Range<usize>> = Vec::new();
        let mut at = 0;
        while at + n.len() <= hay.len() {
            if &hay[at..at + n.len()] == n {
                out.push(at..at + n.len());
                at += n.len();
            } else {
                at += 1;
            }
        }
        out
    })
}

/// Ten numbered lines, line 4 changed to `LINE 4 line`.
fn one_change() -> Spec {
    let old = numbered("line", 10);
    let mut new = old.clone();
    new[4] = "LINE 4 line\n".to_owned();
    Spec::modified("src/a.rs", &old.concat(), &new.concat())
}

fn set(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
    highlights: Option<FindHighlights>,
) {
    view.update(cx, |v, cx| v.set_find_highlights(highlights, cx));
    settle(cx);
}

/// `(x, y, width)` of the quads of `color`, sorted top to bottom.
fn marks(cx: &mut VisualTestContext, color: gpui_kit::Hsla) -> Vec<(f32, f32, f32)> {
    let mut q: Vec<(f32, f32, f32)> = quads_of(cx, color)
        .into_iter()
        .map(|(x, y, w, _)| (x, y, w))
        .collect();
    q.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    q
}

fn theme(view: &Entity<DiffViewport>, cx: &mut VisualTestContext) -> Arc<ViewportTheme> {
    view.read_with(cx, |v, _| v.options().theme.clone())
}

#[gpui_kit::test]
fn find_marks_every_visible_match_and_the_current_one(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let t = theme(&view, cx);
    assert!(
        marks(cx, t.find_match).is_empty(),
        "nothing before a search"
    );

    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line"),
            current: None,
        }),
    );
    // Visible code rows: context 1–3, `-line 4`, `+LINE 4 line`, context
    // 5–7. One match each, two on the added line.
    let d = debug(&view, cx);
    let code_rows: Vec<usize> = (0..d.visible_rows.len())
        .filter(|&i| d.visible_rows[i].starts_with(' '))
        .collect();
    assert_eq!(code_rows.len(), 8, "{:#?}", d.visible_rows);
    let found = marks(cx, t.find_match);
    assert_eq!(found.len(), 9, "{found:?}");
    // Each is four characters wide, on a code row, where its text starts.
    for &(x, y, w) in &found {
        assert!((w - 4.0 * ADVANCE).abs() < 0.6, "width {w}");
        let row = code_rows
            .iter()
            .find(|&&i| (d.row_bounds[i].0 - y).abs() < 0.6)
            .expect("on a code row");
        let texts = texts_in_row(&d, *row);
        let code_x = texts.last().unwrap().0;
        assert!(x >= code_x - 0.6, "{x} before the code at {code_x}");
    }
    // The second match on the added line: 7 characters in.
    let added = code_rows[4];
    let (y, _) = d.row_bounds[added];
    let on_added: Vec<f32> = found
        .iter()
        .filter(|q| (q.1 - y).abs() < 0.6)
        .map(|q| q.0)
        .collect();
    assert_eq!(on_added.len(), 2);
    assert!((on_added[1] - on_added[0] - 7.0 * ADVANCE).abs() < 0.6);

    // Going to it: that one match (not the line's other) is emphasized.
    view.update(cx, |v, cx| {
        v.set_find_current(
            Some(FindCurrent {
                file_idx: 0,
                side: Side::New,
                line: 4,
                range: 7..11,
            }),
            cx,
        )
    });
    settle(cx);
    let current = marks(cx, t.find_match_current);
    assert_eq!(current.len(), 1);
    assert!((current[0].0 - on_added[1]).abs() < 0.6);
    assert!((current[0].1 - y).abs() < 0.6);
    assert_eq!(marks(cx, t.find_match).len(), 8);

    // Cleared: no marks.
    set(&view, cx, None);
    assert!(marks(cx, t.find_match).is_empty());
    assert!(marks(cx, t.find_match_current).is_empty());
    assert!(view.read_with(cx, |v, _| v.find_highlights().is_none()));
}

#[gpui_kit::test]
fn find_marks_both_columns_in_split(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Split), 1000., 600.);
    let t = theme(&view, cx);
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line 5"),
            current: None,
        }),
    );
    // Context line 5 shows on both sides: marked in each column.
    let found = marks(cx, t.find_match);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].1, found[1].1);
    assert!(found[0].0 < 500.0 && found[1].0 >= 500.0, "{found:?}");
    // A match the line does not have marks nothing.
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("nowhere"),
            current: None,
        }),
    );
    assert!(marks(cx, t.find_match).is_empty());
}

#[gpui_kit::test]
fn find_marks_follow_lines_shaped_again(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![one_change()]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let t = theme(&view, cx);
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line"),
            current: None,
        }),
    );
    let before = marks(cx, t.find_match);
    assert_eq!(before.len(), 9);
    // Lines shaped again (a new font, wrapping on and off) are matched
    // again, and the marks land where they did.
    set_options(&view, cx, |o| o.ligatures = true);
    assert_eq!(marks(cx, t.find_match), before);
    set_options(&view, cx, |o| o.style.wrap = true);
    assert_eq!(marks(cx, t.find_match), before);
    set_options(&view, cx, |o| o.style.wrap = false);
    assert_eq!(marks(cx, t.find_match), before);
    // A new matcher replaces the old marks.
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line 4"),
            current: None,
        }),
    );
    assert_eq!(marks(cx, t.find_match).len(), 2);
}

#[gpui_kit::test]
fn find_marks_never_keep_dropped_lines_alive(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // An added file far taller than the window: every line matches.
    let lines = numbered("line", 200).concat();
    let provider = MemProvider::new(vec![Spec::added("src/a.rs", &lines)]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let t = theme(&view, cx);
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line"),
            current: None,
        }),
    );
    let top = marks(cx, t.find_match);
    assert!(!top.is_empty());
    let cache = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| v.debug_find_cache().expect("find is on"))
    };
    let (live, freed) = cache(&view, cx);
    assert_eq!((live, freed), (top.len(), 0), "one cached line per mark");
    // The text cache drops every shaped line (a font change) and the view
    // scrolls on before those lines are shaped again: the find cache still
    // lists them but holds them weakly, so they are freed, not kept alive.
    let mut opts = view.read_with(cx, |v, _| v.options().clone());
    opts.ligatures = true;
    view.update(cx, |v, cx| {
        v.set_options(opts, cx);
        v.scroll_by(2000.0, cx);
    });
    settle(cx);
    let (live, freed) = cache(&view, cx);
    assert!(live > 0, "the lines now shown are cached");
    assert_eq!(freed, top.len(), "the dropped lines are not kept alive");
    // Back at the top the dropped lines are shaped (and matched) again, and
    // the marks land where they did.
    view.update(cx, |v, cx| v.scroll_by(-2000.0, cx));
    settle(cx);
    assert_eq!(marks(cx, t.find_match), top);
    let (_, freed) = cache(&view, cx);
    assert_eq!(freed, 0, "lines shaped again are matched again");
}

#[gpui_kit::test]
fn find_rects_follow_the_new_code_x(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // With bars the code starts half a column after the numbers: 66.3 px in
    // unified (two 4-column number columns), 35.1 px into each split half
    // and into a one-sided file's single pane.
    let provider = MemProvider::new(vec![one_change(), Spec::added("src/b.rs", "line b\n")]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 800.);
    let t = theme(&view, cx);
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line 1"),
            current: None,
        }),
    );
    let d = debug(&view, cx);
    let (y, _) = d.row_bounds[2];
    let found = marks(cx, t.find_match);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        (found[0].0 - 66.3).abs() <= 0.5 && found[0].1 == y,
        "{found:?}"
    );

    set_options(&view, cx, |o| o.layout = LayoutMode::Split);
    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line 5"),
            current: None,
        }),
    );
    let found = marks(cx, t.find_match);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!((found[0].0 - 35.1).abs() <= 0.5, "{found:?}");
    assert!((found[1].0 - 535.1).abs() <= 0.5, "{found:?}");

    set(
        &view,
        cx,
        Some(FindHighlights {
            matcher: matcher("line b"),
            current: None,
        }),
    );
    let found = marks(cx, t.find_match);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!((found[0].0 - 35.1).abs() <= 0.5, "{found:?}");
}
