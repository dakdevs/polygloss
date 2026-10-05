//! File cards on the canvas and the header-card slot (T6.5, design §11.6,
//! ADR-0027).

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Entity, IntoElement as _, ParentElement as _, Styled as _, TestAppContext, VisualTestContext,
    div, px, size,
};
use polygloss_diff::Side;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{
    BlockAnchor, BlockId, BlockSpec, ControlAction, DiffProvider, DiffViewport, LayoutMode,
    RenderBlock, RowKey, ScrollAnchor, ScrollTarget, ViewportDebug, ViewportEvent, ViewportOptions,
};

use crate::support::*;

// ---------------------------------------------------------------------------
// helpers

/// A card's outer edges in a 1000 px viewport: 16 px from each side.
const CARD: (f32, f32) = (16.0, 968.0);

/// Two added files: `a.rs` (30 lines, a 600 px body) and `b.rs` (`b_lines`
/// lines).
fn two_added(b_lines: u32) -> Arc<MemProvider> {
    MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 30).concat()),
        Spec::added("b.rs", &numbered("b", b_lines).concat()),
    ])
}

/// A prelude that is a plain box as tall as `h` says, as wide as its slot.
fn prelude_box(h: Rc<Cell<f32>>) -> RenderBlock {
    Rc::new(move |_, _| div().w_full().h(px(h.get())).into_any_element())
}

/// Opens a `width × height` window with a viewport over `provider` whose
/// prelude is set before its first frame, and lets everything settle.
fn open_with_prelude(
    cx: &mut TestAppContext,
    provider: Arc<MemProvider>,
    opts: ViewportOptions,
    (width, height): (f32, f32),
    prelude: RenderBlock,
) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    assert_sandboxed();
    let window = cx.open_window(size(px(width), px(height)), move |window, cx| {
        let mut view = DiffViewport::new(provider as Arc<dyn DiffProvider>, opts, window, cx);
        view.set_prelude(Some(prelude), cx);
        view
    });
    let view = window.root(cx).expect("window has a root view");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    settle(cx);
    (view, cx)
}

fn scroll_top(view: &Entity<DiffViewport>, cx: &mut VisualTestContext) -> f64 {
    view.read_with(cx, |v, _| v.document().scroll_top())
}

/// File `f`'s painted header `(y, sticky)`.
fn header_at(d: &ViewportDebug, f: u32) -> (f32, bool) {
    d.headers
        .iter()
        .find(|h| h.file_idx == f)
        .map(|h| (h.y, h.sticky))
        .unwrap_or_else(|| panic!("no header for file {f}: {:?}", d.headers))
}

// ---------------------------------------------------------------------------
// the flat v1 layout

/// The documents `flat_layout_without_cards_paints_as_before` paints: a
/// modified file with gaps and a block, an added file, a binary file and a
/// small modified file.
fn flat_fixture() -> Arc<MemProvider> {
    let old = numbered("line", 60).concat();
    let new = old.replace("line 30\n", "line 30 changed\n");
    MemProvider::new(vec![
        Spec::modified("src/lib.rs", &old, &new),
        Spec::added("new.rs", &numbered("n", 12).concat()),
        Spec::binary("img.png"),
        Spec::modified("tail.rs", "a\nb\n", "a\nc\n"),
    ])
}

/// What a frame painted, as the fixture records it: rows with their bounds,
/// headers, painted text and controls (the anchor is left out: v1's
/// document top was `(0, Header, 0)`).
fn dump(name: &str, d: &ViewportDebug) -> String {
    let mut out = format!("## {name}\nrows:\n");
    for (row, (y, h)) in d.visible_rows.iter().zip(&d.row_bounds) {
        writeln!(out, "  ({y:?}, {h:?}) {row}").unwrap();
    }
    out.push_str("headers:\n");
    for h in &d.headers {
        writeln!(out, "  {h:?}").unwrap();
    }
    out.push_str("texts:\n");
    for (x, y, t) in &d.painted_text {
        writeln!(out, "  ({x:?}, {y:?}) {t}").unwrap();
    }
    out.push_str("controls:\n");
    for c in &d.controls {
        writeln!(out, "  {c:?}").unwrap();
    }
    out
}

fn flat_options(layout: LayoutMode) -> ViewportOptions {
    ViewportOptions {
        cards: None,
        ..options(layout)
    }
}

#[gpui_kit::test]
fn flat_layout_without_cards_paints_as_before(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let mut out = String::new();

    let (view, vcx) = open(
        cx,
        flat_fixture(),
        flat_options(LayoutMode::Unified),
        1000.,
        400.,
    );
    view.update(vcx, |v, cx| {
        v.set_blocks(
            0,
            vec![BlockSpec {
                id: BlockId(7),
                anchor: BlockAnchor::Line {
                    side: Side::New,
                    line: 29,
                },
                render: Rc::new(|_, _| div().h(px(30.)).into_any_element()),
            }],
            cx,
        )
    });
    settle(vcx);
    out += &dump("unified, top", &debug(&view, vcx));
    wheel(vcx, 250.);
    out += &dump("unified, 250 px down", &debug(&view, vcx));
    view.update(vcx, |v, cx| v.set_collapsed(1, true, cx));
    settle(vcx);
    out += &dump("unified, new.rs collapsed", &debug(&view, vcx));
    view.update(vcx, |v, cx| v.scroll_to(ScrollTarget::File(2), cx));
    settle(vcx);
    out += &dump("unified, img.png at the top", &debug(&view, vcx));

    let (view, vcx) = open(
        cx,
        flat_fixture(),
        flat_options(LayoutMode::Split),
        1400.,
        300.,
    );
    out += &dump("split, top", &debug(&view, vcx));
    // The first file's header pushed up by the next one.
    let first = view.read_with(vcx, |v, _| v.document().file_height(0));
    wheel(vcx, first - 30.);
    out += &dump("split, header pushed", &debug(&view, vcx));

    let expected = include_str!("fixtures/flat-layout-v1.txt");
    assert!(
        out == expected,
        "the flat layout changed:\n{}",
        first_difference(expected, &out)
    );
}

/// The first line where `actual` differs from `expected`, with its number.
fn first_difference(expected: &str, actual: &str) -> String {
    let (mut e, mut a) = (expected.lines(), actual.lines());
    for n in 1.. {
        match (e.next(), a.next()) {
            (None, None) => return "(only line endings differ)".to_owned(),
            (x, y) if x == y => continue,
            (x, y) => return format!("line {n}: expected {x:?}, got {y:?}"),
        }
    }
    unreachable!()
}

// ---------------------------------------------------------------------------
// the prelude and the top of the document

#[gpui_kit::test]
fn fresh_document_opens_at_the_top_of_the_lead(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let h = Rc::new(Cell::new(72.0));
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(cx, two_added(20), opts, (1000., 400.), prelude_box(h));
    let d = debug(&view, cx);
    assert_eq!(scroll_top(&view, cx), 0.0);
    assert_eq!(d.anchor, ScrollAnchor::default());
    assert_eq!(d.anchor.row, RowKey::Lead);
    // The prelude's top is the viewport's top edge, a card wide; the first
    // card follows a 12 px gap below it.
    assert_eq!(d.prelude, Some((CARD.0, 0.0, CARD.1, 72.0)));
    assert_eq!(header_at(&d, 0), (84.0, false));
}

#[gpui_kit::test]
fn late_set_prelude_keeps_scroll_top_0(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        two_added(20),
        card_options(LayoutMode::Unified),
        1000.,
        400.,
    );
    let d = debug(&view, cx);
    assert_eq!((d.prelude, header_at(&d, 0)), (None, (0.0, false)));

    let prelude = prelude_box(Rc::new(Cell::new(72.0)));
    view.update(cx, |v, cx| v.set_prelude(Some(prelude), cx));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(scroll_top(&view, cx), 0.0);
    assert_eq!(d.prelude, Some((CARD.0, 0.0, CARD.1, 72.0)));
    assert_eq!(header_at(&d, 0), (84.0, false));

    // Removing it gives the first card the top again.
    view.update(cx, |v, cx| v.set_prelude(None, cx));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!((d.prelude, header_at(&d, 0)), (None, (0.0, false)));
}

#[gpui_kit::test]
fn growing_the_prelude_keeps_scroll_top_0(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let h = Rc::new(Cell::new(70.0));
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(
        cx,
        two_added(20),
        opts,
        (1000., 600.),
        prelude_box(h.clone()),
    );
    assert_eq!(header_at(&debug(&view, cx), 0), (82.0, false));
    // The commit list opens: 70 → 400 px. The prelude grows downward, the
    // top stays put.
    h.set(400.0);
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(scroll_top(&view, cx), 0.0);
    assert_eq!(d.prelude, Some((CARD.0, 0.0, CARD.1, 400.0)));
    assert_eq!(header_at(&d, 0), (412.0, false));
}

/// Below a prelude taller than the viewport only the first file's lead is
/// in view: nothing of its card is painted until its top enters.
#[gpui_kit::test]
fn a_card_below_the_viewport_paints_nothing(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let h = Rc::new(Cell::new(500.0));
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(cx, two_added(20), opts, (1000., 400.), prelude_box(h));
    let d = debug(&view, cx);
    assert_eq!(d.prelude, Some((CARD.0, 0.0, CARD.1, 500.0)));
    assert!(d.headers.is_empty(), "{:?}", d.headers);
    assert!(d.visible_rows.is_empty(), "{:?}", d.visible_rows);
    assert!(d.controls.is_empty(), "{:?}", d.controls);
    // Its header starts at 512 (500 + a 12 px gap): scrolled 113 px, its
    // top is 1 px above the bottom edge.
    wheel(cx, 113.);
    assert_eq!(header_at(&debug(&view, cx), 0), (399.0, false));
}

#[gpui_kit::test]
fn growing_the_prelude_above_a_line_anchor_keeps_that_line(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let h = Rc::new(Cell::new(70.0));
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(
        cx,
        two_added(20),
        opts,
        (1000., 400.),
        prelude_box(h.clone()),
    );
    // Line 3 of a.rs 150 px below the top: its row starts at 82 (lead) +
    // 40 (header) + 40, so the prelude still shows 58 px of itself.
    let anchor = ScrollAnchor {
        file_idx: 0,
        row: RowKey::Line {
            side: Side::New,
            line: 2,
        },
        offset_px: -150.0,
    };
    view.update(cx, |v, cx| v.scroll_to_anchor(anchor, cx));
    settle(cx);
    let line = unified(None, Some(3), '+', "a 2");
    let at = |cx: &mut VisualTestContext| {
        let d = debug(&view, cx);
        let i = d.visible_rows.iter().position(|r| *r == line).unwrap();
        (d.row_bounds[i].0, d.prelude)
    };
    assert_eq!(scroll_top(&view, cx), 12.0);
    assert_eq!(at(cx), (150.0, Some((CARD.0, -12.0, CARD.1, 70.0))));
    h.set(400.0);
    settle(cx);
    // The line did not move; the prelude grew above it.
    assert_eq!(scroll_top(&view, cx), 412.0 + 40.0 + 40.0 - 150.0);
    assert_eq!(at(cx), (150.0, Some((CARD.0, -342.0, CARD.1, 400.0))));
    assert_eq!(view.read_with(cx, |v, _| v.anchor()), anchor);
}

#[gpui_kit::test]
fn prelude_scrolls_with_the_first_card_and_is_measured(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Five 200 px boxes wrap at a card's width (968): two rows, 40 px. At
    // the viewport's width (1000) they would fit in one.
    let prelude: RenderBlock = Rc::new(|_, _| {
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .children((0..5).map(|_| div().w(px(200.)).h(px(20.))))
            .into_any_element()
    });
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(cx, two_added(20), opts, (1000., 400.), prelude);
    let d = debug(&view, cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.document().prelude_height()),
        Some(40.0)
    );
    assert_eq!(d.prelude, Some((CARD.0, 0.0, CARD.1, 40.0)));
    assert_eq!(header_at(&d, 0), (52.0, false));

    wheel(cx, 30.);
    let d = debug(&view, cx);
    assert_eq!(d.prelude, Some((CARD.0, -30.0, CARD.1, 40.0)));
    assert_eq!(header_at(&d, 0), (22.0, false));
    assert_eq!(d.anchor.row, RowKey::Lead);

    // Scrolled past it: it is not placed, and the first header pins.
    wheel(cx, 60.);
    let d = debug(&view, cx);
    assert_eq!(d.prelude, None);
    assert_eq!(header_at(&d, 0), (0.0, true));
}

#[gpui_kit::test]
fn set_provider_keeps_the_prelude(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let h = Rc::new(Cell::new(72.0));
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(cx, two_added(20), opts, (1000., 400.), prelude_box(h));
    wheel(cx, 100.);
    let next = MemProvider::new(vec![Spec::added("c.rs", &numbered("c", 5).concat())]);
    view.update(cx, |v, cx| {
        v.set_provider(next as Arc<dyn DiffProvider>, &[], cx);
        assert_eq!(v.document().prelude_height(), Some(72.0));
        assert_eq!(v.document().scroll_top(), 0.0);
    });
    // The very next frame has it, so the header card never vanishes.
    redraw(cx);
    let d = debug(&view, cx);
    assert_eq!(d.prelude, Some((CARD.0, 0.0, CARD.1, 72.0)));
    assert_eq!(header_at(&d, 0), (84.0, false));
}

// ---------------------------------------------------------------------------
// cards

#[gpui_kit::test]
fn scroll_to_file_puts_the_header_at_the_top_edge(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let h = Rc::new(Cell::new(72.0));
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open_with_prelude(cx, two_added(20), opts, (1000., 400.), prelude_box(h));
    // a.rs: 84 lead + 40 header + 600 body + 8 padding = 732, then 12 px
    // of canvas above b.rs's card.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(1), cx));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(scroll_top(&view, cx), 744.0);
    assert_eq!(d.visible_rows[0], "== b.rs");
    assert_eq!(header_at(&d, 1), (0.0, false));
    assert_eq!(d.anchor.row, RowKey::Header);

    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(scroll_top(&view, cx), 84.0);
    assert_eq!(header_at(&d, 0), (0.0, false));
    assert_eq!(d.prelude, None);
}

#[gpui_kit::test]
fn split_threshold_uses_the_inner_width(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Split from 160 columns of 7.8 px = 1248 px. A 1270 px viewport has
    // 162.8 columns, but a card's inner width (1270 − 2 × 17 = 1236) only
    // 158.5.
    let (view, vcx) = open(
        cx,
        two_added(5),
        card_options(LayoutMode::Auto),
        1270.,
        400.,
    );
    assert_eq!(debug(&view, vcx).layout, Layout::Unified);
    // The flat layout's rows span the viewport: split at the same width.
    let flat = flat_options(LayoutMode::Auto);
    let (view, vcx) = open(cx, two_added(5), flat, 1270., 400.);
    assert_eq!(debug(&view, vcx).layout, Layout::Split);

    // 1290 px: the inner 1256 px hold 161 columns.
    let (view, cx) = open(
        cx,
        two_added(5),
        card_options(LayoutMode::Auto),
        1290.,
        400.,
    );
    assert_eq!(debug(&view, cx).layout, Layout::Split);
}

/// A header's content spans its card's inner width: the chevron sits 17 px
/// (a 16 px margin and a 1 px border) right of where it is in the flat
/// layout, Viewed and ⋯ 17 px left of theirs, and clicks there reach them.
#[gpui_kit::test]
fn header_controls_sit_and_take_clicks_inside_the_card(cx: &mut TestAppContext) {
    const INSET: f32 = 17.0;
    let _sb = sandbox();
    let actions = [
        ControlAction::Collapse(0),
        ControlAction::Viewed(0),
        ControlAction::Menu(0),
    ];
    let flat = {
        let opts = options(LayoutMode::Unified);
        let (view, cx) = open(cx, two_added(5), opts, 1000., 800.);
        let d = debug(&view, cx);
        actions.map(|a| control(&d, a))
    };
    let (view, cx) = open(
        cx,
        two_added(5),
        card_options(LayoutMode::Unified),
        1000.,
        800.,
    );
    let (events, _sub) = record_events(&view, cx);
    let d = debug(&view, cx);
    let card = actions.map(|a| control(&d, a));
    let shifted = |(x, y, w, h): (f32, f32, f32, f32), dx: f32| (x + dx, y, w, h);
    assert_eq!(
        card,
        [
            shifted(flat[0], INSET),
            shifted(flat[1], -INSET),
            shifted(flat[2], -INSET),
        ]
    );

    // Clicks at the flat layout's centers, moved by the inset.
    let click = |cx: &mut VisualTestContext, (x, y, w, h): (f32, f32, f32, f32), dx: f32| {
        click_at(cx, x + w / 2.0 + dx, y + h / 2.0)
    };
    click(cx, flat[1], -INSET);
    let toggled: Vec<ViewportEvent> = events
        .borrow()
        .iter()
        .filter(|e| !matches!(e, ViewportEvent::FrameStats(_)))
        .cloned()
        .collect();
    assert_eq!(toggled, [ViewportEvent::ViewedToggled(0)]);
    click(cx, flat[0], INSET);
    assert_eq!(view.read_with(cx, |v, _| v.collapsed()), [0]);
}

#[gpui_kit::test]
fn card_quads_are_clamped_to_the_viewport(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::added("big.rs", &numbered("x", 500).concat())]);
    let opts = card_options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let cards = |cx: &mut VisualTestContext| -> Vec<ShapedQuad> {
        shaped_quads(cx)
            .into_iter()
            .filter(|q| q.fill == Some(theme.card_background) && q.border == theme.card_border)
            .filter(|q| q.bounds.3 > HEADER_H)
            .collect()
    };
    let card = |bounds| ShapedQuad {
        bounds,
        clip: (0.0, 0.0, 1000.0, 400.0),
        fill: Some(theme.card_background),
        radii: [8.0; 4],
        borders: [1.0; 4],
        border: theme.card_border,
    };
    // The canvas first, then the card: from its top to 10 px (radius + 2)
    // past the viewport's bottom edge.
    let all = shaped_quads(cx);
    assert_eq!(all[0].bounds, (0.0, 0.0, 1000.0, 400.0));
    assert_eq!(all[0].fill, Some(theme.canvas));
    assert_eq!(cards(cx), [card((CARD.0, 0.0, CARD.1, 410.0))]);
    // In the middle of its 10,000 px body: 10 px past both edges.
    wheel(cx, 3000.);
    assert_eq!(cards(cx), [card((CARD.0, -10.0, CARD.1, 420.0))]);
    // At the end: its bottom (40 + 10,000 + 8) and 12 px of canvas below.
    wheel(cx, 10_000.);
    assert_eq!(scroll_top(&view, cx), 10_060.0 - 400.0);
    assert_eq!(cards(cx), [card((CARD.0, -10.0, CARD.1, 388.0 + 10.0))]);
}
