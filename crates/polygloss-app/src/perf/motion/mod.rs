//! `motion`: the app shell's motion metrics (T7.3, design §12.1, ADR-0030
//! rule 12), measured in the full app over a corpus review through its real
//! startup. A frame's draw time is [`frames`]'s: `MainWindow::render`'s
//! start to the paint of its last child.
//!
//! Once the review has settled, the run:
//!
//! 1. **Idle** ([`IDLE`]): redraws the window on every display frame with
//!    nothing changing ([`force_frames`]: the root view is notified, as a
//!    running motion's owner is, so cached views are reused). Its p95 is
//!    `shell_idle_draw_p95_ms`, the chrome every motion frame pays (target
//!    ≤ 2 ms, reported); the median interval is the display period.
//! 2. **Drivers** ([`DRIVERS`], one per animated surface): [`ROUNDS`]
//!    open-close rounds each, under the Full override ([`run_drivers`]).
//!    Each toggle's input runs at the start of a display frame; its first
//!    frame is the commit frame, and every later frame until none comes for
//!    [`QUIET`] is an animation frame.
//!
//! Metrics, per surface (null until its driver exists; [`metrics`]):
//! `<name>_anim_draw_p95_ms` and `<name>_anim_draw_max_ms` (animation frames
//! only), `<name>_commit_ms` (input to the end of the commit frame, p95 over
//! opens and closes) and `<name>_late_frames` (intervals between a toggle's
//! frames over 1.5 display periods).

pub mod frames;

use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{AnyWindowHandle, App, AsyncApp, EntityId, Window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::rows::Layout;
use polygloss_viewport::ViewportEvent;
use serde_json::{Map, Value, json};

use crate::motion::{self, MotionPolicy, MotionPolicyOverride};
use crate::perf::{
    Clock, CorpusSpec, PerfArgs, ScenarioResult, finish, max_rss_mb, ms, p95, run_dir,
};
use crate::review_tab;
use crate::settings::SettingsStore;
use crate::settings::model::LayoutSetting;
use crate::startup::{self, Launch};
use frames::{Frame, FrameRecorder};

/// One animated surface: `open` and `close` commit its change as a pointer
/// does (`Initiator::Pointer`); the scenario runs them under the Full
/// override.
pub struct Driver {
    pub name: &'static str,
    pub open: fn(&mut Window, &mut App),
    pub close: fn(&mut Window, &mut App),
}

/// One driver per animated surface, appended by the task that animates it:
/// card (T7.8), section (T7.11), sidebar and threads (T7.10), accordion
/// (T7.13).
pub const DRIVERS: &[Driver] = &[];

/// The surfaces whose metrics `benches/budgets.json` registers: each is
/// reported, null until its driver exists, because run-perf requires every
/// metric of the scenario.
pub const SURFACES: &[&str] = &["sidebar", "threads", "card", "accordion", "section"];

/// Open-close rounds per driver and layout.
pub const ROUNDS: usize = 20;

/// How long the idle phase forces frames.
pub const IDLE: Duration = Duration::from_secs(2);

/// No frame for this long ends a toggle (a motion requests a frame every
/// display frame until it settles).
pub const QUIET: Duration = Duration::from_millis(250);

/// How often a phase checks what was drawn (frames are stamped as they
/// paint, not by this poll).
const POLL: Duration = Duration::from_millis(2);

/// The longest a toggle may take, input to quiet.
const TOGGLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest wait for the review to open and settle.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

/// An interval longer than this many display periods is a late frame.
const LATE_PERIODS: f64 = 1.5;

/// One toggle of a driver: when its input ran and every frame drawn after
/// it until [`QUIET`]; the first is the commit frame.
pub struct Toggle {
    pub input: Instant,
    pub frames: Vec<Frame>,
}

/// One driver's rounds.
pub struct DriverRun {
    pub name: &'static str,
    pub toggles: Vec<Toggle>,
    /// Toggles that drew no frame.
    pub timeouts: usize,
}

/// Notifies `root` (the window's root view) at the start of every display
/// frame until `until` on the executor clock, so each frame redraws the
/// window; it requests no frame after that.
pub fn force_frames(root: EntityId, until: Instant, window: &mut Window, _: &mut App) {
    window.on_next_frame(move |window, cx| {
        if cx.background_executor().now() < until {
            cx.notify(root);
            force_frames(root, until, window, cx);
        }
    });
}

/// The idle phase: [`force_frames`] for `duration`; the frames it drew.
async fn idle(
    window: AnyWindowHandle,
    recorder: &FrameRecorder,
    duration: Duration,
    cx: &mut AsyncApp,
) -> anyhow::Result<Vec<Frame>> {
    let from = recorder.frames().len();
    let until = cx.background_executor().now() + duration;
    window.update(cx, |root, window, cx| {
        force_frames(root.entity_id(), until, window, cx)
    })?;
    while cx.background_executor().now() < until {
        cx.background_executor().timer(POLL).await;
    }
    Ok(recorder
        .since(from)
        .into_iter()
        .filter(|f| f.at < until)
        .collect())
}

/// Runs `rounds` open-close rounds of each driver under the Full override,
/// then puts back the override it found.
pub async fn run_drivers(
    window: AnyWindowHandle,
    recorder: &FrameRecorder,
    drivers: &'static [Driver],
    rounds: usize,
    cx: &mut AsyncApp,
) -> anyhow::Result<Vec<DriverRun>> {
    let previous = cx.update(|cx| cx.try_global::<MotionPolicyOverride>().and_then(|o| o.0));
    cx.update(|cx| motion::set_override(Some(MotionPolicy::Full), cx));
    let mut runs = Vec::new();
    let outcome = async {
        for driver in drivers {
            let mut run = DriverRun {
                name: driver.name,
                toggles: Vec::new(),
                timeouts: 0,
            };
            for _ in 0..rounds {
                for change in [driver.open, driver.close] {
                    match toggle(window, recorder, change, cx).await? {
                        Some(toggle) => run.toggles.push(toggle),
                        None => run.timeouts += 1,
                    }
                }
            }
            runs.push(run);
        }
        anyhow::Ok(())
    }
    .await;
    cx.update(|cx| motion::set_override(previous, cx));
    outcome.map(|()| runs)
}

/// Runs `change` at the start of the next display frame and collects the
/// frames until none comes for [`QUIET`]; `None` when it drew nothing.
async fn toggle(
    window: AnyWindowHandle,
    recorder: &FrameRecorder,
    change: fn(&mut Window, &mut App),
    cx: &mut AsyncApp,
) -> anyhow::Result<Option<Toggle>> {
    let mark = recorder.frames().len();
    let input = Rc::new(Cell::new(None::<Instant>));
    let stamp = input.clone();
    window.update(cx, |_, window, _| {
        window.on_next_frame(move |window, cx| {
            stamp.set(Some(cx.background_executor().now()));
            change(window, cx);
        })
    })?;
    let asked = cx.background_executor().now();
    loop {
        cx.background_executor().timer(POLL).await;
        let now = cx.background_executor().now();
        let Some(input) = input.get() else {
            anyhow::ensure!(
                now.saturating_duration_since(asked) < TOGGLE_TIMEOUT,
                "no display frame ran the input"
            );
            continue;
        };
        let frames: Vec<Frame> = recorder
            .since(mark)
            .into_iter()
            .filter(|f| f.at >= input)
            .collect();
        let last = frames.last().map_or(input, |f| f.at);
        if now.saturating_duration_since(last) >= QUIET {
            return Ok((!frames.is_empty()).then_some(Toggle { input, frames }));
        }
        anyhow::ensure!(
            now.saturating_duration_since(input) < TOGGLE_TIMEOUT,
            "the motion never settled"
        );
    }
}

/// The display period: the median interval between the idle frames.
fn display_period(idle: &[Frame]) -> Option<Duration> {
    let mut intervals: Vec<Duration> = idle
        .windows(2)
        .map(|w| w[1].at.saturating_duration_since(w[0].at))
        .collect();
    intervals.sort();
    intervals.get(intervals.len().checked_sub(1)? / 2).copied()
}

/// Draw times in milliseconds.
fn draw_ms<'a>(frames: impl Iterator<Item = &'a Frame>) -> Vec<f64> {
    frames.map(|f| ms(f.draw)).collect()
}

/// A driver's animation frames: every toggle's frames after its commit
/// frame.
fn animation_frames(toggles: &[Toggle]) -> impl Iterator<Item = &Frame> {
    toggles.iter().flat_map(|t| t.frames.iter().skip(1))
}

/// Each toggle's input to the end of its commit frame, in milliseconds.
fn commit_ms(toggles: &[Toggle]) -> Vec<f64> {
    toggles
        .iter()
        .map(|t| ms(t.frames[0].at.saturating_duration_since(t.input)))
        .collect()
}

/// The scenario's metrics: `shell_idle_draw_p95_ms` and each surface's
/// (every one of [`SURFACES`], plus any other driver's), null where nothing
/// was measured.
pub fn metrics(idle: &[Frame], runs: &[DriverRun]) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "shell_idle_draw_p95_ms".into(),
        json!(p95(&draw_ms(idle.iter()))),
    );
    let late_after = display_period(idle).map(|p| p.as_secs_f64() * LATE_PERIODS);
    let extra = runs
        .iter()
        .map(|r| r.name)
        .filter(|n| !SURFACES.contains(n));
    for name in SURFACES.iter().copied().chain(extra) {
        let toggles = runs
            .iter()
            .find(|r| r.name == name)
            .map_or(&[][..], |r| &r.toggles);
        let anim = draw_ms(animation_frames(toggles));
        let late = late_after.filter(|_| !toggles.is_empty()).map(|limit| {
            toggles
                .iter()
                .flat_map(|t| t.frames.windows(2))
                .filter(|w| w[1].at.saturating_duration_since(w[0].at).as_secs_f64() > limit)
                .count()
        });
        let max = anim.iter().copied().reduce(f64::max);
        m.insert(format!("{name}_anim_draw_p95_ms"), json!(p95(&anim)));
        m.insert(format!("{name}_anim_draw_max_ms"), json!(max));
        m.insert(format!("{name}_commit_ms"), json!(p95(&commit_ms(toggles))));
        m.insert(format!("{name}_late_frames"), json!(late));
    }
    m
}

/// What the frame callback saw of the review tab.
#[derive(Default)]
struct Run {
    files: usize,
    /// The viewport's last frame had no loading and no plain rows.
    settled: bool,
    last_frame: Option<Instant>,
}

/// `Polygloss --perf-scenario motion`: opens the corpus review through the
/// app's startup, waits for it to settle, then measures (module docs).
pub fn run(args: PerfArgs, spec: CorpusSpec, clock: Clock) -> ExitCode {
    let paths = match run_dir() {
        Ok((_, paths)) => paths,
        Err(err) => finish(Err(err), &args),
    };
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
        urls: Vec::new(),
        open: Some(req),
        paths: Some(paths),
        clock,
        before_window: Some(Box::new(move |cx: &mut App| {
            // The layout is pinned like `polygloss-perf`'s.
            let mut settings = SettingsStore::global(cx).settings().clone();
            settings.diff.layout = match before_args.layout {
                Layout::Split => LayoutSetting::Split,
                Layout::Unified => LayoutSetting::Unified,
            };
            SettingsStore::set(settings, cx);
            let state = before_state.clone();
            review_tab::on_new_tab(cx, move |tab, _window, cx| {
                state.borrow_mut().files = tab.read(cx).opened.files.len();
                let viewport = tab.read(cx).viewport.clone();
                let state = state.clone();
                cx.subscribe(&viewport, move |_, event: &ViewportEvent, _| {
                    if let ViewportEvent::FrameStats(stats) = event {
                        let mut s = state.borrow_mut();
                        s.settled = stats.loading_rows == 0 && stats.unhighlighted_rows == 0;
                        s.last_frame = Some(Instant::now());
                    }
                })
                .detach();
            });
        })),
        after_launch: Some(Box::new(move |launched, cx: &mut App| {
            let args = after_args.clone();
            let Some(opening) = launched.opening else {
                finish(Err(anyhow::anyhow!("the window did not open")), &args);
            };
            let window = launched.window;
            let state = after_state.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                if let Err(err) = opening.await {
                    finish(Err(err.context("opening the corpus")), &args);
                }
                let outcome = measure(window, &state, cx).await;
                let result = outcome.map(|m| result(&args, &state.borrow(), m, clock));
                finish(result, &args)
            })
            .detach();
        })),
    })
}

/// What one run measured.
struct Measured {
    idle: Vec<Frame>,
    runs: Vec<DriverRun>,
    window: Value,
}

async fn measure(
    window: AnyWindowHandle,
    state: &Rc<RefCell<Run>>,
    cx: &mut AsyncApp,
) -> anyhow::Result<Measured> {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        let quiet = {
            let s = state.borrow();
            s.settled && s.last_frame.is_some_and(|at| at.elapsed() >= QUIET)
        };
        if quiet {
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "the review did not settle");
        cx.background_executor().timer(POLL).await;
    }
    let (recorder, active_before) = window.update(cx, |_, window, cx| {
        (FrameRecorder::start(window, cx), window.is_window_active())
    })?;
    let idle = idle(window, &recorder, IDLE, cx).await?;
    let runs = run_drivers(window, &recorder, DRIVERS, ROUNDS, cx).await?;
    let window = window.update(cx, |_, window, _| {
        let size = window.viewport_size();
        json!({
            "width": f32::from(size.width),
            "height": f32::from(size.height),
            "scale": window.scale_factor(),
            "active": [active_before, window.is_window_active()],
        })
    })?;
    Ok(Measured { idle, runs, window })
}

fn result(args: &PerfArgs, run: &Run, measured: Measured, clock: Clock) -> ScenarioResult {
    let mut result = ScenarioResult::new(args);
    result.metrics = metrics(&measured.idle, &measured.runs);
    result
        .samples
        .insert("idle_draw_ms".into(), json!(draw_ms(measured.idle.iter())));
    let mut drivers = Map::new();
    for r in &measured.runs {
        let anim = draw_ms(animation_frames(&r.toggles));
        result
            .samples
            .insert(format!("{}_anim_draw_ms", r.name), json!(anim));
        result.samples.insert(
            format!("{}_commit_ms", r.name),
            json!(commit_ms(&r.toggles)),
        );
        drivers.insert(
            r.name.into(),
            json!({
                "toggles": r.toggles.len(),
                "timeouts": r.timeouts,
                "anim_frames": anim.len(),
            }),
        );
    }
    result.info.insert("files".into(), json!(run.files));
    result.info.insert("rounds".into(), json!(ROUNDS));
    result
        .info
        .insert("idle_frames".into(), json!(measured.idle.len()));
    result.info.insert(
        "display_period_ms".into(),
        json!(display_period(&measured.idle).map(ms)),
    );
    result.info.insert("drivers".into(), Value::Object(drivers));
    result.info.insert("window".into(), measured.window);
    result.info.insert("max_rss_mb".into(), json!(max_rss_mb()));
    result.info.insert(
        "process_start".into(),
        json!(if clock.from_kernel { "kernel" } else { "main" }),
    );
    result
}
