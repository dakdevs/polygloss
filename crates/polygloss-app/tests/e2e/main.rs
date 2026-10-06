//! The single E2E test binary of `polygloss-app` (feature `e2e`): GPUI E2E and
//! screenshot suites, one module per feature. `scripts/test-e2e.sh` runs it via
//! `cargo nextest run -p polygloss-app --features e2e -E 'binary(e2e)'`.
//!
//! The binary is `harness = false`: tests are plain `fn()`s listed in each
//! module's `TESTS` table (`tests![…]` names them after the function) and
//! run on the main thread by `support::harness`, because GPUI's real text
//! system can only be created there. `#[test]` and `#[gpui_kit::test]` do
//! nothing in this binary.

#[path = "../support/mod.rs"]
mod support;

mod block_geometry;
mod card_geometry;
mod categories;
mod cursor;
mod feed;
mod filmstrip;
mod find;
mod header_card;
mod home;
mod iterations;
mod keyboard_only_review;
mod kit_fonts;
mod ligatures;
mod live;
mod open_flow;
mod overlay_geometry;
mod palette;
mod press_ink;
mod shell;
mod submit;
mod theme;
mod threads;
mod tree;
mod viewport_screenshots;

use support::harness::Test;

fn e2e_harness_runs_tests_on_the_main_thread() {
    assert_eq!(std::thread::current().name(), Some("main"));
    assert_eq!(
        support::harness::current_test(),
        Some("e2e_harness_runs_tests_on_the_main_thread")
    );
}

const HARNESS: &[Test] = &crate::tests![e2e_harness_runs_tests_on_the_main_thread];

fn main() -> std::process::ExitCode {
    support::harness::run(&[
        HARNESS,
        kit_fonts::TESTS,
        ligatures::TESTS,
        viewport_screenshots::TESTS,
        shell::TESTS,
        palette::TESTS,
        home::TESTS,
        open_flow::TESTS,
        theme::TESTS,
        tree::TESTS,
        threads::TESTS,
        submit::TESTS,
        cursor::TESTS,
        live::TESTS,
        find::TESTS,
        iterations::TESTS,
        feed::TESTS,
        keyboard_only_review::TESTS,
        header_card::TESTS,
        categories::TESTS,
        press_ink::TESTS,
        filmstrip::TESTS,
        card_geometry::TESTS,
        overlay_geometry::TESTS,
        block_geometry::TESTS,
    ])
}
