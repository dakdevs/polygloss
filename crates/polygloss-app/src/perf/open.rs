//! `open`: `app_first_paint_ms`, T2.9's `first_paint_ms` measured in the app
//! itself. Process start → the first frame of the review tab's viewport in
//! which no visible row is still loading (plain text allowed), through the
//! app's real startup ([`crate::startup::run`] with the corpus as the
//! review named on the command line): the store, gpui-kit, the window, the
//! background `Core::open` and the first loads are all inside it.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{App, AsyncApp};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{FrameStats, ViewportEvent};
use serde_json::json;

use crate::perf::{Clock, CorpusSpec, PerfArgs, ScenarioResult, max_rss_mb, ms, timeline_ms};
use crate::review_tab;
use crate::settings::SettingsStore;
use crate::settings::model::LayoutSetting;
use crate::startup::{self, Launch, StartupMarks};

/// Longest wait for first paint (the Linux budget is 2 s).
const TIMEOUT: Duration = Duration::from_secs(120);

/// What the run saw so far.
#[derive(Default)]
struct Run {
    marks: Option<StartupMarks>,
    review_opened: Option<Instant>,
    viewport_attached: Option<Instant>,
    files: usize,
    frames: Vec<(Instant, FrameStats)>,
}

/// First paint among `frames` (in order): the first frame without loading
/// rows. `(first paint, first frame, frames up to first paint)`.
pub fn first_paint(frames: &[(Instant, FrameStats)]) -> Option<(Instant, Instant, usize)> {
    let first = frames.first()?.0;
    let (i, (at, _)) = frames
        .iter()
        .enumerate()
        .find(|(_, (_, stats))| stats.loading_rows == 0)?;
    Some((*at, first, i + 1))
}

pub fn run(args: PerfArgs, spec: CorpusSpec, clock: Clock) -> ExitCode {
    let req = OpenRequest {
        worktree: spec.repo.clone(),
        source: Source::Compare {
            base: spec.base.clone(),
            head: spec.head.clone(),
            mode: if spec.direct {
                CompareMode::Direct
            } else {
                CompareMode::ThreeDot
            },
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let args = Rc::new(args);
    let state = Rc::new(RefCell::new(Run::default()));
    let (before_args, before_state) = (args.clone(), state.clone());
    let (after_args, after_state) = (args.clone(), state.clone());
    startup::run(Launch {
        open: Some(req),
        clock,
        before_window: Some(Box::new(move |cx: &mut App| {
            // The layout is pinned like `polygloss-perf`'s.
            let mut settings = SettingsStore::global(cx).settings().clone();
            settings.diff.layout = match before_args.layout {
                Layout::Split => LayoutSetting::Split,
                Layout::Unified => LayoutSetting::Unified,
            };
            SettingsStore::set(settings, cx);
            let (args, state) = (before_args.clone(), before_state.clone());
            review_tab::on_new_tab(cx, move |tab, _window, cx| {
                let viewport = tab.read(cx).viewport.clone();
                {
                    let mut s = state.borrow_mut();
                    s.viewport_attached = Some(Instant::now());
                    s.files = tab.read(cx).opened.files.len();
                }
                let (args, state) = (args.clone(), state.clone());
                cx.subscribe(&viewport, move |_, event: &ViewportEvent, _cx| {
                    let ViewportEvent::FrameStats(stats) = event else {
                        return;
                    };
                    let painted = {
                        let mut s = state.borrow_mut();
                        s.frames.push((Instant::now(), *stats));
                        stats.loading_rows == 0
                    };
                    if painted {
                        finish(result(&args, &state.borrow(), clock), &args);
                    }
                })
                .detach();
            });
        })),
        after_launch: Some(Box::new(move |launched, cx: &mut App| {
            after_state.borrow_mut().marks = Some(launched.marks);
            let args = after_args.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                cx.background_executor().timer(TIMEOUT).await;
                finish(
                    Err(anyhow::anyhow!("no first paint within {TIMEOUT:?}")),
                    &args,
                )
            })
            .detach();
            let Some(opening) = launched.opening else {
                finish(Err(anyhow::anyhow!("the window did not open")), &after_args);
            };
            let (args, state) = (after_args.clone(), after_state.clone());
            cx.spawn(async move |_: &mut AsyncApp| match opening.await {
                Ok(_) => state.borrow_mut().review_opened = Some(Instant::now()),
                Err(err) => finish(Err(err.context("opening the corpus")), &args),
            })
            .detach();
        })),
    })
}

/// The result once first paint was seen.
fn result(args: &PerfArgs, run: &Run, clock: Clock) -> anyhow::Result<ScenarioResult> {
    let (painted, first_frame, frames) =
        first_paint(&run.frames).ok_or_else(|| anyhow::anyhow!("no first paint"))?;
    let since = |t: Instant| ms(t.saturating_duration_since(clock.process_start));
    let mut result = ScenarioResult::new(args);
    result
        .metrics
        .insert("app_first_paint_ms".into(), json!(since(painted)));
    result.info.insert("files".into(), json!(run.files));
    result.info.insert(
        "first_paint".into(),
        json!({
            "first_frame_ms": since(first_frame),
            "frames": frames,
        }),
    );
    let marks = run.marks;
    result.info.insert(
        "timeline_ms".into(),
        timeline_ms(
            clock.process_start,
            &[
                ("main", Some(clock.main)),
                ("app_launched", marks.map(|m| m.app_launched)),
                ("kit_initialized", marks.map(|m| m.kit_initialized)),
                ("window_opened", marks.map(|m| m.window_opened)),
                ("viewport_attached", run.viewport_attached),
                ("review_opened", run.review_opened),
                ("first_frame", Some(first_frame)),
                ("first_paint", Some(painted)),
            ],
        ),
    );
    result.info.insert("max_rss_mb".into(), json!(max_rss_mb()));
    result.info.insert(
        "process_start".into(),
        json!(if clock.from_kernel { "kernel" } else { "main" }),
    );
    Ok(result)
}

/// Prints the result (or the error) and exits: 0 with a result, 1 on
/// failure. GPUI's macOS run loop never returns, so the process exits here.
fn finish(outcome: anyhow::Result<ScenarioResult>, args: &PerfArgs) -> ! {
    use std::io::Write as _;
    let code = match outcome {
        Ok(result) => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{}", result.render(args.json).trim_end());
            let _ = stdout.flush();
            0
        }
        Err(err) => {
            eprintln!(
                "Polygloss --perf-scenario {} {}: {err:#}",
                args.scenario.as_str(),
                args.corpus
            );
            1
        }
    };
    std::process::exit(code)
}
