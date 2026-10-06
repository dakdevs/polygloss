//! A file card's edges against the reference (T7.4, ADR-0031 R5–R12): a
//! one-sided `internal/diff/diff.go` whose first line starts `//`, with
//! 3-digit line numbers, at the default 13 pt in Polygloss Light, drawn by
//! Metal. Origins are layout bounds (the viewport's `card_bounds`, its debug
//! frame); glyph and icon ink is `support::ink`, in bands that start inside
//! the card's border (`card_bounds` + `BORDER`). Each edge is asserted as
//! `abs(measured − reference) ≤ allowed + 0.5`, both numbers written here
//! from ADR-0031's reference-edges table. R2 and R3 (the card on the main
//! column) are layout only: `tests/app/card_geometry.rs`, where the
//! `main-column` selector can be read.

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Bounds, Entity, HeadlessAppContext, point, px, size};
use image::RgbaImage;
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::tabs::TabItem;
use polygloss_app::{startup, window};
use polygloss_core::git::Source;
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;
use polygloss_viewport::ControlAction;

use crate::support::harness::Test;
use crate::support::ink::Ink;
use crate::support::screenshot::{self, SCALE, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_card_edges_meet_the_reference];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 30;

/// `internal/diff/diff.go` as the reference shows it, then enough lines for
/// 3-digit numbers.
fn diff_go() -> String {
    let mut text = String::from(
        "// Package diff holds the model of a set of changed files and parses the\n\
         // patches git prints into it.\n\
         package diff\n",
    );
    for i in 3..120 {
        text.push_str(&format!("var v{i} = {i}\n"));
    }
    text
}

/// `godiff`: a first commit with a README, then `init` adding `diff.go`.
fn godiff_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("README.md", b"# godiff\n");
    repo.commit("readme");
    repo.write("internal/diff/diff.go", diff_go().as_bytes());
    repo.commit("init");
    repo
}

/// The app at 1280×800 showing `init` once its rows are highlighted.
fn open_settled(repo: &FixtureRepo) -> (HeadlessAppContext, AnyWindowHandle, Entity<ReviewTab>) {
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit { rev: "HEAD".into() },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        let Some(tab) = tab else { continue };
        let debug = cx.update(|cx| tab.read(cx).viewport.read(cx).debug());
        if debug.visible_rows.len() > 10
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
        {
            for _ in 0..3 {
                screenshot::draw(&mut cx, handle);
            }
            return (cx, handle, tab);
        }
    }
    panic!("the review tab never settled");
}

/// `abs(measured − reference) ≤ allowed + 0.5`. Prints each measurement
/// for the gate's review (`--no-capture`).
fn assert_edge(name: &str, measured: Option<f32>, reference: f32, allowed: f32) {
    let measured = measured.unwrap_or_else(|| panic!("{name}: no ink in its band"));
    eprintln!("{name}: {measured} pt (the reference's {reference} ± {allowed})");
    assert!(
        (measured - reference).abs() <= allowed + 0.5,
        "{name}: measured {measured}, the reference's {reference} ± {allowed}"
    );
}

/// A band in whole points, rounded inward: `x0..x1` × `y0..y1`.
fn band(x0: f32, x1: f32, y0: f32, y1: f32) -> Bounds<u32> {
    let (x0, y0) = (x0.ceil() as u32, y0.ceil() as u32);
    let (x1, y1) = (x1.floor() as u32, y1.floor() as u32);
    Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0))
}

fn e2e_card_edges_meet_the_reference() {
    let _sb = Sandbox::isolate();
    let repo = godiff_repo();
    let (mut cx, handle, tab) = open_settled(&repo);
    let (card, d, pill_background) = cx.update(|cx| {
        let v = tab.read(cx).viewport.read(cx);
        let f = v.display_order()[0];
        let card = v.card_bounds(f).expect("the card is painted");
        (card, v.debug(), v.options().theme.pill_background)
    });
    assert!(
        d.headers[0].title == "internal/diff/diff.go",
        "{:?}",
        d.headers
    );
    let image: RgbaImage = screenshot::capture(&mut cx, handle);
    // The card in window points, and the viewport's origin (the debug
    // frame is viewport-relative).
    let (left, top) = (card.left().as_f32(), card.top().as_f32());
    let origin = (left - d.cards[0].bounds.0, top - d.cards[0].bounds.1);
    let inner = left + 1.0;
    let ink = Ink::default();

    // R10: the card's top to its first row (the header's two borders and
    // its interior), from the debug frame.
    let first_row = origin.1 + d.row_bounds[1].0;
    assert_edge("R10 file header", Some(first_row - top), 46.0, 0.0);
    let header = (top + 1.0, first_row - 1.0);
    let row = (first_row, first_row + d.row_bounds[1].1);

    // R5: the change bar, from the card's inner edge: the band starts on
    // the bar, so its end is the first "ink".
    let bar_end = ink.first_x(&image, band(inner, inner + 20.0, row.0 + 2.0, row.1 - 2.0));
    assert_edge("R5 change bar", bar_end.map(|x| x - inner), 4.0, 0.0);

    // R6, R7: the open chevron's ink and the path's (`internal/…`), from
    // the card's outer edge.
    let chevron = ink.first_x(&image, band(inner, inner + 30.0, header.0, header.1));
    assert_edge("R6 chevron", chevron.map(|x| x - left), 15.5, 0.5);
    let path = ink.first_x(
        &image,
        band(inner + 32.0, inner + 120.0, header.0, header.1),
    );
    assert_edge("R7 path", path.map(|x| x - left), 42.0, 1.0);

    // R8: the gutter tint's end, past the numbers (right-aligned 8 pt
    // before it) and before the code. The tint and the row's are 13 apart
    // in one channel, under the glyph threshold: this band uses 8.
    let tint = Ink { threshold: 8 };
    let gutter_end = tint.first_x(
        &image,
        band(inner + 33.0, inner + 48.0, row.0 + 2.0, row.1 - 2.0),
    );
    assert_edge("R8 gutter tint", gutter_end.map(|x| x - left), 42.5, 1.5);

    // R9: the code's ink (`//`), from the card's outer edge.
    let code = ink.first_x(&image, band(inner + 41.0, inner + 80.0, row.0, row.1));
    assert_edge("R9 code", code.map(|x| x - left), 53.0, 1.0);

    // R11: the `+a −d` pill's height, from the painted scene.
    let pill = cx
        .update_window(handle, |_, window, _| {
            window.painted_quads().into_iter().find(|q| {
                let y = q.bounds.origin.y.0 / SCALE as f32;
                q.background.as_solid() == Some(pill_background) && y > header.0 && y < header.1
            })
        })
        .expect("the window is open")
        .expect("the counts pill");
    let pill_h = pill.bounds.size.height.0 / SCALE as f32;
    assert_edge("R11 pill", Some(pill_h), 22.0, 2.0);

    // R12: the Viewed control, from the debug frame.
    let viewed = d
        .controls
        .iter()
        .find(|c| matches!(c.action, ControlAction::Viewed(_)))
        .expect("Viewed");
    assert_edge("R12 Viewed", Some(viewed.bounds.3), 30.0, 2.0);
}
