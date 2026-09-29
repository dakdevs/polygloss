//! `polygloss-perf`: headed perf scenarios over the design §12.2 corpora (plan
//! T2.9; dev only, never shipped).
//!
//! ```text
//! polygloss-perf --corpus <name> --layout split|unified --scenario open|scroll|highlight|blocks --json
//! ```
//!
//! One scenario per process: open the corpus through `polygloss-core` in a
//! private temp store, open a window with a [`DiffViewport`] (Pierre Light,
//! syntax on, the layout pinned), run the scenario (see [`scenarios`]) and
//! print one result object ([`metrics::ScenarioResult`]). Exit codes: 0 with
//! a result, 1 when the corpus cannot be opened or the scenario fails, 2 for
//! usage errors. `benches/run-perf.ts` runs the whole matrix.
//!
//! [`DiffViewport`]: polygloss_viewport::DiffViewport

mod args;
mod corpus;
mod harness;
mod metrics;
mod scenarios;

use std::cell::RefCell;
use std::io::Write as _;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{App, AsyncApp};
use polygloss_diff::rows::Layout;
use polygloss_highlight::Appearance;
use polygloss_viewport::{LayoutMode, ViewportOptions, ViewportTheme};
use serde_json::json;

use crate::args::{Args, USAGE};
use crate::corpus::OpenedCorpus;
use crate::harness::Harness;
use crate::metrics::{Clock, ScenarioResult, max_rss_mb, ms};

/// A run that takes longer than this fails (run-perf kills at 15 min).
const WATCHDOG: Duration = Duration::from_secs(10 * 60);

fn main() -> ExitCode {
    let clock = Clock::now();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if args::is_version_request(&argv) {
        println!("polygloss-perf {}", polygloss_core::VERSION);
        return ExitCode::SUCCESS;
    }
    let args = match Args::parse(argv) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("polygloss-perf: {err}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let lookup = Instant::now();
    let spec = match &args.entry {
        Some(spec) => spec.clone(),
        None => match corpus::manifest_entry(&args.corpus) {
            Ok(spec) => spec,
            Err(err) => {
                eprintln!("polygloss-perf: {err:#}");
                return ExitCode::from(1);
            }
        },
    };
    // Looking the corpus up is harness work, not the app's.
    let overhead = if args.entry.is_none() {
        lookup.elapsed()
    } else {
        Duration::ZERO
    };
    let corpus = match corpus::open_corpus(&spec) {
        Ok(corpus) => corpus,
        Err(err) => {
            eprintln!("polygloss-perf: {err:#}");
            return ExitCode::from(1);
        }
    };
    run_app(args, corpus, clock, overhead)
}

/// The viewport as the gate measures it: the layout pinned, Pierre Light,
/// syntax on, every other option at its default.
fn viewport_options(layout: Layout) -> ViewportOptions {
    ViewportOptions {
        layout: match layout {
            Layout::Split => LayoutMode::Split,
            Layout::Unified => LayoutMode::Unified,
        },
        theme: Arc::new(ViewportTheme::pierre(Appearance::Light)),
        syntax: true,
        ..ViewportOptions::default()
    }
}

fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::Split => "split",
        Layout::Unified => "unified",
    }
}

/// One run: what was asked, its clock, and the corpus it opened.
struct Run {
    args: Args,
    clock: Clock,
    /// Harness time inside the measured interval (the manifest lookup).
    overhead: Duration,
    /// When the corpus was open, the app had launched and gpui-kit was
    /// initialized (the timeline).
    corpus_opened: Instant,
    app_launched: Instant,
    kit_initialized: Instant,
    /// Taken (and its temp dir deleted) by [`finish`].
    corpus: RefCell<Option<OpenedCorpus>>,
}

/// Runs the scenario in a GPUI app and exits the process when it is done.
fn run_app(args: Args, corpus: OpenedCorpus, clock: Clock, overhead: Duration) -> ExitCode {
    let corpus_opened = Instant::now();
    gpui_kit::application().run(move |cx: &mut App| {
        let app_launched = Instant::now();
        // The header's ⋯ menu is a gpui-kit `PopupMenu` (T2.5); named fonts,
        // so gpui-kit does not scan the installed ones (T2.10.2).
        polygloss_viewport::kit::init_kit(&viewport_options(args.layout).code_font, cx);
        let run = Rc::new(Run {
            args,
            clock,
            overhead,
            corpus_opened,
            app_launched,
            kit_initialized: Instant::now(),
            corpus: RefCell::new(Some(corpus)),
        });
        let Some(provider) = run.corpus.borrow().as_ref().map(|c| c.provider.clone()) else {
            return;
        };
        let title = format!(
            "polygloss-perf · {} · {} · {}",
            run.args.scenario.as_str(),
            run.args.corpus,
            layout_name(run.args.layout)
        );
        let options = viewport_options(run.args.layout);
        let mut harness = match Harness::open(cx, provider, options, &title) {
            Ok(h) => h,
            Err(err) => finish(Err(err.context("opening the window")), &run),
        };
        cx.activate(true);

        let watchdog = run.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor().timer(WATCHDOG).await;
            let err = anyhow::anyhow!("the scenario did not finish within {WATCHDOG:?}");
            finish(Err(err), &watchdog)
        })
        .detach();

        cx.spawn(async move |cx: &mut AsyncApp| {
            let args = &run.args;
            let mut result = ScenarioResult::new(
                args.scenario.as_str(),
                &args.corpus,
                layout_name(args.layout),
            );
            let outcome =
                scenarios::run(&mut harness, cx, args, run.clock, run.overhead, &mut result).await;
            let outcome = outcome.map(|()| {
                describe(&mut result, &run, &harness, cx);
                result
            });
            finish(outcome, &run)
        })
        .detach();
    });
    // `run` does not return on macOS (the app exits through `finish`).
    ExitCode::from(1)
}

/// Context every result carries.
fn describe(result: &mut ScenarioResult, run: &Run, harness: &Harness, cx: &mut AsyncApp) {
    let (args, clock) = (&run.args, run.clock);
    if let Some(c) = run.corpus.borrow().as_ref() {
        result.info("files", json!(c.opened.files.len()));
        result.info("store_ms", json!(ms(c.store_time)));
        result.info("open_ms", json!(ms(c.open_time)));
    }
    // Where the time before first paint goes, from process start.
    let since = |t: Instant| ms(t.saturating_duration_since(clock.process_start));
    result.info(
        "timeline_ms",
        json!({
            "main": since(clock.main),
            "corpus_opened": since(run.corpus_opened),
            "app_launched": since(run.app_launched),
            "kit_initialized": since(run.kit_initialized),
            "window_opened": since(harness.opened_at),
            "first_frame": harness.seen.first().map(|f| since(f.at)),
        }),
    );
    if let Some(spec) = &args.entry {
        result.info(
            "corpus",
            json!({
                "repo": spec.repo,
                "base": spec.base,
                "head": spec.head,
                "mode": if spec.direct { "direct" } else { "three-dot" },
            }),
        );
    }
    if let Ok((width, height, scale)) = harness.window_metrics(cx) {
        result.info(
            "window",
            json!({ "width": width, "height": height, "scale": scale }),
        );
    }
    let stats = harness.read(cx, |v| v.pipeline_stats());
    result.info(
        "pipeline",
        json!({
            "loads": stats.loads,
            "highlights": stats.highlights,
            "cancelled": stats.cancelled,
            "token_cache_hits": stats.token_cache_hits,
        }),
    );
    result.info("frames", json!(harness.seen.len()));
    result.info("max_rss_mb", json!(max_rss_mb()));
    result.info("seed", json!(args.knobs.seed));
    if !args.knobs.is_default() {
        result.info("knobs", json!(format!("{:?}", args.knobs)));
    }
    result.info(
        "process_start",
        json!(if clock.from_kernel { "kernel" } else { "main" }),
    );
    result.info("harness_overhead_ms", json!(ms(run.overhead)));
}

/// Prints the result (or the error), deletes the run's private store and
/// exits: 0 with a result, 1 on failure.
fn finish(outcome: anyhow::Result<ScenarioResult>, run: &Run) -> ! {
    let args = &run.args;
    let code = match outcome {
        Ok(result) => {
            let text = if args.json {
                result.to_json_line()
            } else {
                result.to_human()
            };
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{}", text.trim_end());
            let _ = stdout.flush();
            0
        }
        Err(err) => {
            eprintln!(
                "polygloss-perf: {} {} {}: {err:#}",
                args.scenario.as_str(),
                args.corpus,
                layout_name(args.layout)
            );
            1
        }
    };
    if let Some(c) = run.corpus.borrow_mut().take() {
        c.close();
    }
    std::process::exit(code)
}
