//! The header card against the reference (T7.7, ADR-0031 R13–R15): the
//! commit `init`, at the default 13 pt in Polygloss Light, drawn by Metal.
//! The origin is the card's layout bounds, borders included: the header
//! card is an app element, the viewport's prelude, and a headless window
//! exposes no debug selectors, so its bounds are the viewport debug frame's
//! prelude (`tests/app/header_card.rs` `header_card_is_56_tall` checks that
//! they are the `header-card` selector's). Glyph and avatar ink is
//! `support::ink`, in bands that start inside the card's border. Each edge
//! is asserted as `abs(measured − reference) ≤ allowed + 0.5`, both numbers
//! written here from ADR-0031's reference-edges table.

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

use crate::support::harness::Test;
use crate::support::ink::Ink;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_header_card_meets_the_reference];

/// Frames drawn at most while waiting for loads.
const MAX_FRAMES: usize = 30;

/// `godiff`: a README, then `init` adding `internal/diff/diff.go`.
fn godiff_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("README.md", b"# godiff\n");
    repo.commit("readme");
    repo.write(
        "internal/diff/diff.go",
        b"// Package diff holds the model of a set of changed files.\npackage diff\n",
    );
    repo.commit("init");
    repo
}

/// The app at its screenshot size showing `init` once its header card is
/// placed.
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
        if debug.prelude.is_some_and(|p| p.3 > 0.0) && !debug.cards.is_empty() {
            for _ in 0..3 {
                screenshot::draw(&mut cx, handle);
            }
            return (cx, handle, tab);
        }
    }
    panic!("the header card never landed");
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

fn e2e_header_card_meets_the_reference() {
    let _sb = Sandbox::isolate();
    let repo = godiff_repo();
    let (mut cx, handle, tab) = open_settled(&repo);
    let (prelude, origin) = cx.update(|cx| {
        let v = tab.read(cx).viewport.read(cx);
        let d = v.debug();
        let at = &d.cards[0];
        let card = v
            .card_bounds(at.file_idx)
            .expect("the file card is painted");
        (
            d.prelude.expect("the header card is placed"),
            (
                card.left().as_f32() - at.bounds.0,
                card.top().as_f32() - at.bounds.1,
            ),
        )
    });
    let image: RgbaImage = screenshot::capture(&mut cx, handle);
    // The header card in window points, borders included.
    let (left, top) = (origin.0 + prelude.0, origin.1 + prelude.1);
    let bottom = top + prelude.3;
    let inner = left + 1.0;
    let ink = Ink::default();

    // R13: the card's height, borders included.
    assert_edge("R13 header card", Some(prelude.3), 56.5, 0.5);

    // R14: the avatar's left edge, from the card's outer left edge (the
    // band stops before the title's column).
    let avatar = ink.first_x(&image, band(inner, inner + 44.0, top + 1.0, bottom - 1.0));
    assert_edge("R14 avatar", avatar.map(|x| x - left), 17.0, 0.5);

    // R15: the title's ink (`init`), from the card's outer left edge: a
    // band right of the avatar, over the title's line (below the top
    // inset, above the byline).
    let title = ink.first_x(
        &image,
        band(inner + 44.0, inner + 200.0, top + 9.0, top + 29.0),
    );
    assert_edge("R15 title", title.map(|x| x - left), 53.5, 1.5);
}
