//! The file cards on the main column (T7.4, ADR-0031 M1, R2 and R3),
//! measured from layout bounds: the `main-column` and `header-card` debug
//! selectors and the viewport's `card_bounds`. The reference's ink edges
//! (R5–R12) are the E2E's (`tests/e2e/card_geometry.rs`): only a real
//! renderer draws glyphs, and a headless window has no debug selectors.

use gpui_kit::TestAppContext;

use crate::shell::{bounds, commit_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

/// `abs(measured − reference) ≤ allowed + 0.5`, ADR-0031's reference test.
fn assert_edge(name: &str, measured: f32, reference: f32, allowed: f32) {
    assert!(
        (measured - reference).abs() <= allowed + 0.5,
        "{name}: measured {measured}, the reference's {reference} ± {allowed}"
    );
}

#[gpui_kit::test]
fn cards_sit_on_the_main_columns_edge(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // A commit's review: the header card above the first file card.
    let tab = shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .expect("open the review");
    draw(shell.cx);
    let main = bounds(shell.cx, "main-column");
    let header_card = bounds(shell.cx, "header-card");
    let card = tab
        .read_with(shell.cx, |t, cx| {
            let v = t.viewport.read(cx);
            v.card_bounds(v.display_order()[0])
        })
        .expect("the first file card is painted");
    let (card_left, card_right) = (card.left().as_f32(), card.right().as_f32());
    assert_edge("R2 left", card_left - main.left().as_f32(), 12.0, 0.0);
    assert_edge("R2 right", main.right().as_f32() - card_right, 12.0, 0.0);
    // The header card is a card: it takes a card's edges and the gap.
    assert_eq!(
        (header_card.left().as_f32(), header_card.right().as_f32()),
        (card_left, card_right)
    );
    let gap = card.top().as_f32() - header_card.bottom().as_f32();
    assert_edge("R3", gap, 12.0, 0.0);
}
