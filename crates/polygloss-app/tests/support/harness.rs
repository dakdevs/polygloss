//! A libtest-compatible runner for the `e2e` test binary (`harness = false`).
//!
//! Pixel screenshots need GPUI's real macOS text system, and gpui-kit can only
//! create it on the process's main thread (`MacPlatform::new` asserts that),
//! while libtest runs every test on a thread of its own. So
//! `tests/e2e/main.rs` lists its tests in tables of [`Test`] and calls
//! [`run`], which runs the selected tests one after another **on the main
//! thread**. It speaks the part of libtest's command line that cargo and
//! nextest use:
//!
//! | Arguments                                              | Meaning                                                        |
//! | ------------------------------------------------------ | -------------------------------------------------------------- |
//! | `--list [--format terse] [--ignored]`                  | one `<name>: test` line per (ignored) test, as nextest expects |
//! | `[FILTER…] [--exact] [--skip F] [--ignored \| --include-ignored]` | run tests whose name contains a filter (equals it with `--exact`) |
//! | `--nocapture`, `-q`, `--test-threads N`, `--color C`, … | accepted and ignored (output is never captured)                |
//!
//! Anything else is an error (exit 2), so a new nextest flag fails loudly
//! instead of silently selecting the wrong tests. A test fails by panicking;
//! the exit status is 101 when any test failed, like libtest. nextest runs
//! each test in its own process (`--exact <name> --nocapture`), so the
//! per-process sandbox rule holds; plain `cargo test` runs them in one
//! process, one after another.

use std::cell::RefCell;
use std::process::ExitCode;
use std::time::Instant;

/// One E2E test.
#[derive(Clone, Copy)]
pub struct Test {
    pub name: &'static str,
    pub run: fn(),
    /// Listed but only run with `--ignored` or `--include-ignored`.
    pub ignored: bool,
}

impl Test {
    pub const fn new(name: &'static str, run: fn()) -> Test {
        Test {
            name,
            run,
            ignored: false,
        }
    }

    pub const fn ignored(name: &'static str, run: fn()) -> Test {
        Test {
            name,
            run,
            ignored: true,
        }
    }
}

/// `[Test::new("a", a), Test::new("b", b)]` from `tests![a, b]`: the name is
/// always the function's own.
#[macro_export]
macro_rules! tests {
    ($($name:ident),* $(,)?) => {
        [$($crate::support::harness::Test::new(stringify!($name), $name)),*]
    };
}

/// Which tests `--ignored` / `--include-ignored` select.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ignored {
    /// Run tests that are not ignored (the default).
    #[default]
    Skip,
    /// `--ignored`: only ignored tests.
    Only,
    /// `--include-ignored`: all tests.
    Include,
}

/// A parsed command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invocation {
    pub list: bool,
    /// `--format terse` (nextest's listing format).
    pub terse: bool,
    pub filters: Vec<String>,
    pub skip: Vec<String>,
    pub exact: bool,
    pub ignored: Ignored,
}

/// Options that take a value (`--opt v` or `--opt=v`) and are ignored.
const IGNORED_WITH_VALUE: [&str; 5] = [
    "--test-threads",
    "--color",
    "--logfile",
    "--shuffle-seed",
    "-Z",
];

/// Flags that are accepted and ignored.
const IGNORED_FLAGS: [&str; 10] = [
    "--nocapture",
    "--no-capture",
    "-q",
    "--quiet",
    "--show-output",
    "--test",
    "--report-time",
    "--ensure-time",
    "--shuffle",
    "--force-run-in-process",
];

/// Parses libtest's command line (without the program name).
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut inv = Invocation::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--list" => inv.list = true,
            "--exact" => inv.exact = true,
            "--ignored" => inv.ignored = Ignored::Only,
            "--include-ignored" => inv.ignored = Ignored::Include,
            "--skip" => inv.skip.push(value(flag)?),
            "--format" => match value(flag)?.as_str() {
                "terse" => inv.terse = true,
                "pretty" => inv.terse = false,
                other => return Err(format!("unsupported --format {other}")),
            },
            f if IGNORED_WITH_VALUE.contains(&f) => {
                value(f)?;
            }
            f if IGNORED_FLAGS.contains(&f) => {}
            f if f.starts_with('-') => return Err(format!("unknown option {arg}")),
            _ => inv.filters.push(arg.clone()),
        }
    }
    Ok(inv)
}

fn name_matches(name: &str, pattern: &str, exact: bool) -> bool {
    if exact {
        name == pattern
    } else {
        name.contains(pattern)
    }
}

/// Whether `test` passes the name filters and `--skip`s (ignoring
/// `--ignored`).
fn named(test: &Test, inv: &Invocation) -> bool {
    (inv.filters.is_empty()
        || inv
            .filters
            .iter()
            .any(|f| name_matches(test.name, f, inv.exact)))
        && !inv
            .skip
            .iter()
            .any(|s| name_matches(test.name, s, inv.exact))
}

/// The tests that run, in table order.
pub fn select<'a>(tests: &'a [Test], inv: &Invocation) -> Vec<&'a Test> {
    tests
        .iter()
        .filter(|t| named(t, inv))
        .filter(|t| match inv.ignored {
            Ignored::Skip => !t.ignored,
            Ignored::Only => t.ignored,
            Ignored::Include => true,
        })
        .collect()
}

/// `--list` output: every test that passes the name filters (only ignored
/// ones with `--ignored`), with libtest's summary unless `--format terse`.
pub fn list(tests: &[Test], inv: &Invocation) -> String {
    let listed: Vec<&Test> = tests
        .iter()
        .filter(|t| named(t, inv))
        .filter(|t| inv.ignored != Ignored::Only || t.ignored)
        .collect();
    let mut out: String = listed
        .iter()
        .map(|t| format!("{}: test\n", t.name))
        .collect();
    if !inv.terse {
        out.push_str(&format!("\n{} tests, 0 benchmarks\n", listed.len()));
    }
    out
}

thread_local! {
    static CURRENT: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

/// The name of the test [`run`] is running on this thread (the screenshot
/// runner names baselines after it).
pub fn current_test() -> Option<&'static str> {
    CURRENT.with(|c| *c.borrow())
}

/// The `e2e` binary's `main`: parses `std::env::args`, then lists or runs.
pub fn run(suites: &[&[Test]]) -> ExitCode {
    let tests: Vec<Test> = suites.iter().flat_map(|s| s.iter().copied()).collect();
    let mut seen = std::collections::HashSet::new();
    for t in &tests {
        assert!(seen.insert(t.name), "duplicate E2E test name {}", t.name);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let inv = match parse(&args) {
        Ok(inv) => inv,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(2);
        }
    };
    if inv.list {
        print!("{}", list(&tests, &inv));
        return ExitCode::SUCCESS;
    }

    let selected = select(&tests, &inv);
    let ignored = tests
        .iter()
        .filter(|t| named(t, &inv) && t.ignored && inv.ignored == Ignored::Skip)
        .count();
    let filtered_out = tests.len() - selected.len() - ignored;
    let started = Instant::now();
    println!(
        "\nrunning {} test{}",
        selected.len(),
        if selected.len() == 1 { "" } else { "s" }
    );
    let mut failed = Vec::new();
    for test in &selected {
        CURRENT.with(|c| *c.borrow_mut() = Some(test.name));
        let outcome = std::panic::catch_unwind(test.run);
        CURRENT.with(|c| *c.borrow_mut() = None);
        let ok = outcome.is_ok();
        println!(
            "test {} ... {}",
            test.name,
            if ok { "ok" } else { "FAILED" }
        );
        if !ok {
            failed.push(test.name);
        }
    }
    if !failed.is_empty() {
        println!("\nfailures:");
        for name in &failed {
            println!("    {name}");
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed; {} ignored; 0 measured; {} filtered out; finished in {:.2}s\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len(),
        ignored,
        filtered_out,
        started.elapsed().as_secs_f64()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(101)
    }
}
