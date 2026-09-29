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

mod cursor;
mod find;
mod home;
mod kit_fonts;
mod open_flow;
mod palette;
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
        find::TESTS,
    ])
}
