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

use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext as _, AsyncApp};
use polygloss_diff::rows::Layout;
use polygloss_highlight::Appearance;
use polygloss_viewport::{LayoutMode, ViewportOptions, ViewportTheme};
use serde_json::json;

use crate::args::{Args, USAGE};
use crate::corpus::{OpenedCorpus, RunDir};
use crate::harness::{Harness, PerfWindow};
use crate::metrics::{Clock, ScenarioResult, max_rss_mb, ms, timeline_ms};

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
    // The run's private store's dir, made before GPUI starts so that every
    // exit from the app (`finish`) deletes it, also one before the corpus
    // is open.
    let dir = match RunDir::create() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("polygloss-perf: {err:#}");
            return ExitCode::from(1);
        }
    };
    run_app(args, spec, dir, clock, overhead)
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

/// One run: what was asked, its clock, its private store and the corpus it
/// opened there.
struct Run {
    args: Args,
    clock: Clock,
    /// Harness time inside the measured interval (the manifest lookup).
    overhead: Duration,
    /// When the app had launched, gpui-kit was initialized and the corpus
    /// was open (the timeline).
    app_launched: Instant,
    kit_initialized: Instant,
    corpus_opened: Cell<Option<Instant>>,
    /// Set once the corpus is open; dropped by [`finish`].
    corpus: RefCell<Option<OpenedCorpus>>,
    /// The run's temp dir, the store's home; taken and deleted by
    /// [`finish`], whether or not the corpus opened.
    dir: RefCell<Option<RunDir>>,
}

/// Runs the scenario in a GPUI app and exits the process when it is done.
///
/// Startup is laid out like the app's (design §6.2: git never runs on the
/// UI thread): once the app has launched, the corpus opens on the background
/// executor (resolve, `diff-tree`, the store) while gpui-kit initializes and
/// the window opens, and the viewport goes into the window when the file
/// list is there (plan T2.10.3).
fn run_app(
    args: Args,
    spec: corpus::CorpusSpec,
    dir: RunDir,
    clock: Clock,
    overhead: Duration,
) -> ExitCode {
    gpui_kit::application().run(move |cx: &mut App| {
        let app_launched = Instant::now();
        let root = dir.path().to_owned();
        let opening = cx.background_spawn(async move {
            let corpus = corpus::open_corpus(&spec, &root);
            (corpus, Instant::now())
        });
        let options = viewport_options(args.layout);
        // The header's ⋯ menu is a gpui-kit `PopupMenu` (T2.5); named fonts,
        // so gpui-kit does not scan the installed ones (T2.10.2).
        polygloss_viewport::kit::init_kit(&options.code_font, cx);
        let run = Rc::new(Run {
            args,
            clock,
            overhead,
            app_launched,
            kit_initialized: Instant::now(),
            corpus_opened: Cell::new(None),
            corpus: RefCell::new(None),
            dir: RefCell::new(Some(dir)),
        });
        let title = format!(
            "polygloss-perf · {} · {} · {}",
            run.args.scenario.as_str(),
            run.args.corpus,
            layout_name(run.args.layout)
        );
        let window = match PerfWindow::open(cx, &title) {
            Ok(w) => w,
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
            let (corpus, opened_at) = opening.await;
            let corpus = match corpus {
                Ok(corpus) => corpus,
                Err(err) => finish(Err(err), &run),
            };
            let provider = corpus.provider.clone();
            run.corpus_opened.set(Some(opened_at));
            *run.corpus.borrow_mut() = Some(corpus);
            let mut harness = match Harness::attach(window, cx, provider, options) {
                Ok(h) => h,
                Err(err) => finish(Err(err.context("opening the viewport")), &run),
            };
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
    // Where the time before first paint goes, from process start (the
    // corpus opens while gpui-kit initializes and the window opens).
    result.info(
        "timeline_ms",
        timeline_ms(
            clock.process_start,
            &[
                ("main", Some(clock.main)),
                ("app_launched", Some(run.app_launched)),
                ("kit_initialized", Some(run.kit_initialized)),
                ("window_opened", Some(harness.opened_at)),
                ("corpus_opened", run.corpus_opened.get()),
                ("viewport_attached", Some(harness.attached_at)),
                ("first_frame", harness.seen.first().map(|f| f.at)),
            ],
        ),
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
    drop(run.corpus.borrow_mut().take());
    if let Some(dir) = run.dir.borrow_mut().take() {
        dir.close();
    }
    std::process::exit(code)
}
