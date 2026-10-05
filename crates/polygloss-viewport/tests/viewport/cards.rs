//! File cards on the canvas and the header-card slot (T6.5, design §11.6,
//! ADR-0027).

use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{IntoElement as _, Styled as _, TestAppContext, div, px};
use polygloss_diff::Side;
use polygloss_viewport::{
    BlockAnchor, BlockId, BlockSpec, LayoutMode, ScrollTarget, ViewportDebug, ViewportOptions,
};

use crate::support::*;

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
    options(layout)
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
