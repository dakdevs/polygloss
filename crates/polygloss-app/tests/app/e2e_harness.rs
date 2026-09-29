//! The `e2e` binary's runner (`support::harness`): the libtest command line
//! that cargo and nextest use, and test selection. That it really runs tests
//! on the main thread is checked inside the `e2e` binary itself
//! (`e2e_harness_runs_tests_on_the_main_thread`).

use crate::support::harness::{Ignored, Invocation, Test, list, parse, select};
use crate::support::strings;

fn noop() {}

const TESTS: &[Test] = &[
    Test::new("e2e_viewport_split_pierre_light", noop),
    Test::new("e2e_viewport_unified_pierre_dark", noop),
    Test::new("e2e_viewport_special_files", noop),
    Test::ignored("e2e_slow_scenario", noop),
];

fn names(tests: &[&Test]) -> Vec<&'static str> {
    tests.iter().map(|t| t.name).collect()
}

#[test]
fn harness_parses_the_nextest_command_lines() {
    assert_eq!(
        parse(&strings(["--list", "--format", "terse"])).unwrap(),
        Invocation {
            list: true,
            terse: true,
            ..Invocation::default()
        }
    );
    assert_eq!(
        parse(&strings(["--list", "--format=terse", "--ignored"])).unwrap(),
        Invocation {
            list: true,
            terse: true,
            ignored: Ignored::Only,
            ..Invocation::default()
        }
    );
    assert_eq!(
        parse(&strings([
            "--exact",
            "e2e_viewport_special_files",
            "--nocapture",
            "--include-ignored",
        ]))
        .unwrap(),
        Invocation {
            exact: true,
            filters: vec!["e2e_viewport_special_files".into()],
            ignored: Ignored::Include,
            ..Invocation::default()
        }
    );
}

#[test]
fn harness_accepts_and_ignores_display_options() {
    let inv = parse(&strings([
        "split",
        "--test-threads",
        "4",
        "--color=never",
        "-q",
        "--skip",
        "light",
        "--show-output",
    ]))
    .unwrap();
    assert_eq!(
        inv,
        Invocation {
            filters: vec!["split".into()],
            skip: vec!["light".into()],
            ..Invocation::default()
        }
    );
}

#[test]
fn harness_rejects_unknown_options_and_missing_values() {
    assert!(parse(&strings(["--frobnicate"])).is_err());
    assert!(parse(&strings(["--skip"])).is_err());
    assert!(parse(&strings(["--format", "json"])).is_err());
}

#[test]
fn harness_lists_tests_in_libtest_terse_format() {
    let all = parse(&strings(["--list", "--format", "terse"])).unwrap();
    assert_eq!(
        list(TESTS, &all),
        "e2e_viewport_split_pierre_light: test\n\
         e2e_viewport_unified_pierre_dark: test\n\
         e2e_viewport_special_files: test\n\
         e2e_slow_scenario: test\n"
    );
    let ignored = parse(&strings(["--list", "--format", "terse", "--ignored"])).unwrap();
    assert_eq!(list(TESTS, &ignored), "e2e_slow_scenario: test\n");
    let pretty = parse(&strings(["--list"])).unwrap();
    assert!(list(TESTS, &pretty).ends_with("\n4 tests, 0 benchmarks\n"));
}

#[test]
fn harness_selects_by_substring_exact_name_skip_and_ignored() {
    let run = |args: &[&str]| names(&select(TESTS, &parse(&strings(args)).unwrap()));
    assert_eq!(
        run(&[]),
        [
            "e2e_viewport_split_pierre_light",
            "e2e_viewport_unified_pierre_dark",
            "e2e_viewport_special_files",
        ]
    );
    assert_eq!(
        run(&["pierre"]),
        [
            "e2e_viewport_split_pierre_light",
            "e2e_viewport_unified_pierre_dark",
        ]
    );
    assert!(run(&["--exact", "pierre"]).is_empty());
    assert_eq!(
        run(&["--exact", "e2e_viewport_special_files"]),
        ["e2e_viewport_special_files"]
    );
    assert_eq!(
        run(&["viewport", "--skip", "dark"]),
        [
            "e2e_viewport_split_pierre_light",
            "e2e_viewport_special_files",
        ]
    );
    assert_eq!(run(&["--ignored"]), ["e2e_slow_scenario"]);
    assert_eq!(run(&["slow", "--include-ignored"]), ["e2e_slow_scenario"]);
    assert!(run(&["slow"]).is_empty());
}
