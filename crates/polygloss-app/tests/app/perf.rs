//! `Polygloss --perf-scenario` (T3.1, plan T2.9, OQ-P4): arguments, the
//! corpus manifest entry and first paint from a run's frames.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use polygloss_app::perf::open::first_paint;
use polygloss_app::perf::{CorpusSpec, PerfArgs, Scenario, parse_manifest_entry};
use polygloss_diff::rows::Layout;
use polygloss_viewport::FrameStats;

use crate::support::strings;

#[test]
fn perf_args_parse_the_run_perf_command_line() {
    let parse = |args: &[&str]| PerfArgs::parse(&strings(args));
    let args = parse(&[
        "open", "--corpus", "linux", "--layout", "unified", "--json", "--repo", "/c/linux",
        "--base", "v6.10", "--head", "v6.11", "--direct",
    ])
    .unwrap();
    assert_eq!(
        args,
        PerfArgs {
            scenario: Scenario::Open,
            corpus: "linux".into(),
            layout: Layout::Unified,
            json: true,
            entry: Some(CorpusSpec {
                repo: PathBuf::from("/c/linux"),
                base: "v6.10".into(),
                head: "v6.11".into(),
                direct: true,
            }),
        }
    );
    // T7.3's scenario.
    let motion = parse(&[
        "motion", "--corpus", "linux", "--layout", "unified", "--json",
    ])
    .unwrap();
    assert_eq!(motion.scenario, Scenario::Motion);
    assert_eq!(Scenario::Motion.as_str(), "motion");
    // T3.10's scenario.
    let roundtrip = parse(&["comment-roundtrip", "--corpus", "synthetic", "--json"]).unwrap();
    assert_eq!(roundtrip.scenario, Scenario::CommentRoundtrip);
    assert_eq!(Scenario::CommentRoundtrip.as_str(), "comment-roundtrip");
    let lookup = parse(&["open", "--corpus", "typical"]).unwrap();
    assert_eq!(
        (lookup.layout, lookup.json, lookup.entry),
        (Layout::Split, false, None)
    );
    for bad in [
        &[][..],
        &["nope", "--corpus", "typical"],
        &["open"],
        &["open", "--corpus", "typical", "--repo", "/r"],
        &["open", "--corpus", "typical", "--direct"],
        &["open", "--corpus", "typical", "--layout", "diagonal"],
        &["open", "--corpus", "typical", "--frobnicate"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    let spec = parse_manifest_entry(
        r#"{"name":"typical","repo":"/c/t","base":"corpus-base","head":"corpus-head","mode":"three-dot"}"#,
    )
    .unwrap();
    assert_eq!(
        spec,
        CorpusSpec {
            repo: PathBuf::from("/c/t"),
            base: "corpus-base".into(),
            head: "corpus-head".into(),
            direct: false,
        }
    );
    assert!(parse_manifest_entry(r#"{"repo":"/c"}"#).is_err());
}

#[test]
fn app_first_paint_is_the_first_frame_without_loading_rows() {
    let start = Instant::now();
    let frame = |ms: u64, loading_rows: u32| {
        (
            start + Duration::from_millis(ms),
            FrameStats {
                prepaint: Duration::from_millis(1),
                paint: Duration::from_millis(1),
                visible_rows: 40,
                shaped_lines: 40,
                loading_rows,
                unhighlighted_rows: loading_rows,
            },
        )
    };
    let frames = [frame(90, 12), frame(110, 3), frame(140, 0), frame(150, 0)];
    let (painted, first, n) = first_paint(&frames).unwrap();
    assert_eq!(painted - start, Duration::from_millis(140));
    assert_eq!(first - start, Duration::from_millis(90));
    assert_eq!(n, 3);
    assert!(first_paint(&frames[..2]).is_none());
    assert!(first_paint(&[]).is_none());
}

#[test]
fn perf_runs_keep_every_path_in_their_private_dir() {
    let sb = crate::support::Sandbox::isolate();
    let root = tempfile::tempdir().unwrap();
    let paths = polygloss_app::perf::private_paths(root.path()).unwrap();
    for (name, path) in [
        ("data_dir", &paths.data_dir),
        ("db", &paths.db),
        ("cache_dir", &paths.cache_dir),
        ("scratch_dir", &paths.scratch_dir),
        ("logs_dir", &paths.logs_dir),
        ("config_dir", &paths.config_dir),
    ] {
        assert!(path.starts_with(root.path()), "{name}: {}", path.display());
        // Not the environment's (the sandbox's) paths.
        assert!(
            !path.starts_with(sb.home()) && !path.starts_with(sb.data_dir()),
            "{name}: {}",
            path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// The motion scenario (T7.3, design §12.1): the frame sentinel, the idle
// phase, the drivers and the metrics.

mod motion_scenario {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use gpui_kit::{
        AnyWindowHandle, App, AppContext as _, Context, Element as _, Global,
        InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, Window, canvas, div, px, rgb,
    };
    use polygloss_app::motion::{
        self, Initiator, Motion, MotionPolicy, MotionPolicyOverride, Reduced, ReducedPlay, Track,
        tokens,
    };
    use polygloss_app::perf::motion::frames::{self, Frame, FrameRecorder, Sentinel};
    use polygloss_app::perf::motion::{self as scenario, Driver, DriverRun, Toggle};
    use serde_json::Value;

    use crate::shell;
    use crate::support::Sandbox;
    use crate::support::motion::requested_frames;

    /// Busy-waits `d` of wall-clock time.
    fn spin(d: Duration) {
        let start = Instant::now();
        while start.elapsed() < d {
            std::hint::spin_loop();
        }
    }

    /// The one-minute load average (`sysctl -n vm.loadavg`), for the
    /// timing-test rule: wall-clock upper bounds are asserted only below 4.
    fn load_average() -> Option<f64> {
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()?;
        String::from_utf8(out.stdout)
            .ok()?
            .split_whitespace()
            .find_map(|w| w.parse::<f64>().ok())
    }

    /// A window root built like `MainWindow::render`: it marks its start,
    /// spins in its render, then in a child's paint, then the sentinel,
    /// then a sibling that paints after the sentinel.
    struct Timed {
        render: Duration,
        paint: Duration,
        after: Duration,
    }

    impl Render for Timed {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            frames::render_started(window, cx);
            spin(self.render);
            let (paint, after) = (self.paint, self.after);
            div()
                .size_full()
                .child(canvas(|_, _, _| {}, move |_, _, _, _| spin(paint)).size_full())
                .child(frames::sentinel())
                .child(canvas(|_, _, _| {}, move |_, _, _, _| spin(after)).size_full())
        }
    }

    /// Draws one frame of `cx`'s window.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn timed(cx: &mut TestAppContext, render: u64, paint: u64, after: u64) -> Duration {
        let ms = Duration::from_millis;
        let (view, cx) = cx.add_window_view(|_, _| Timed {
            render: ms(0),
            paint: ms(0),
            after: ms(0),
        });
        draw(cx);
        view.update(cx, |t, _| {
            (t.render, t.paint, t.after) = (ms(render), ms(paint), ms(after))
        });
        let recorder = cx.update(FrameRecorder::start);
        draw(cx);
        let frames = recorder.frames();
        assert_eq!(frames.len(), 1, "one frame drawn, one frame timed");
        frames[0]
    }

    #[gpui_kit::test]
    fn motion_frames_are_timed_render_to_paint(cx: &mut TestAppContext) {
        let paint = timed(cx, 0, 3, 0);
        assert!(paint >= Duration::from_millis(3), "a 3 ms paint: {paint:?}");
        let both = timed(cx, 2, 3, 0);
        assert!(
            both >= Duration::from_millis(5),
            "the render's 2 ms count too: {both:?}"
        );
        // Work painted after the sentinel is not in the frame: an upper
        // bound on wall-clock time, so only on a quiet machine.
        let after = timed(cx, 0, 0, 30);
        match load_average() {
            Some(load) if load < 4.0 => assert!(
                after < Duration::from_millis(30),
                "30 ms painted after the sentinel counted: {after:?} (load {load})"
            ),
            load => eprintln!("after the sentinel: {after:?}; load {load:?}: not asserted"),
        }
    }

    /// A root with one 80 × 40 box, with or without the sentinel; it marks
    /// its render's start only when `marks`.
    struct Inert {
        sentinel: bool,
        marks: bool,
    }

    impl Render for Inert {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.marks {
                frames::render_started(window, cx);
            }
            let root = div().size_full().child(
                div()
                    .debug_selector(|| "inert-box".into())
                    .w(px(80.))
                    .h(px(40.))
                    .bg(rgb(0x3366cc)),
            );
            if self.sentinel {
                root.child(frames::sentinel())
            } else {
                root
            }
        }
    }

    #[gpui_kit::test]
    fn the_sentinel_is_inert_unless_recording(cx: &mut TestAppContext) {
        let _sb = Sandbox::isolate();
        // No element id: the sentinel can hold no element state.
        assert_eq!(Sentinel.id(), None);
        // It lays out and paints nothing: the scene and the box's bounds are
        // the same with it and without it.
        let mut scenes = Vec::new();
        for sentinel in [false, true] {
            let (_, vcx) = cx.add_window_view(|_, _| Inert {
                sentinel,
                marks: true,
            });
            draw(vcx);
            let quads = vcx.update(|window, _| {
                window
                    .painted_quads()
                    .iter()
                    .map(|q| format!("{:?}", q.bounds))
                    .collect::<Vec<_>>()
            });
            let bounds = vcx.debug_bounds("inert-box").expect("the box is painted");
            assert_eq!(requested_frames(vcx), 0, "the sentinel requests no frame");
            scenes.push((quads, bounds));
        }
        assert_eq!(scenes[0], scenes[1]);

        // The main window: nothing is requested or recorded while nobody
        // records it, and every draw is timed while somebody does.
        // A recorder of another window whose root never marks a start: it
        // can only see a frame the main window's sentinel wrongly gave it.
        let elsewhere = {
            let (_, other) = cx.add_window_view(|_, _| Inert {
                sentinel: true,
                marks: false,
            });
            other.update(FrameRecorder::start)
        };
        let shell = shell::start(cx);
        let shell_cx = shell.cx;
        for _ in 0..3 {
            draw(shell_cx);
            assert_eq!(requested_frames(shell_cx), 0, "no frame requested");
        }
        assert!(
            elsewhere.frames().is_empty(),
            "another window's recorder sees none of the main window's frames"
        );
        let recorder = shell_cx.update(FrameRecorder::start);
        for drawn in 1..=3 {
            draw(shell_cx);
            assert_eq!(recorder.frames().len(), drawn, "one timed frame per draw");
            assert_eq!(requested_frames(shell_cx), 0, "recording requests no frame");
        }
        assert_eq!(recorder.painted_at().len(), 3);
        drop(recorder);
        draw(shell_cx);
        assert_eq!(requested_frames(shell_cx), 0);
        assert!(elsewhere.frames().is_empty());
    }

    #[gpui_kit::test]
    fn idle_scenario_requests_only_its_own_frames(cx: &mut TestAppContext) {
        let _sb = Sandbox::isolate();
        let shell = shell::start(cx);
        let vcx = shell.cx;
        let window = vcx.update(|window, _| window.window_handle());
        let recorder = vcx.update(FrameRecorder::start);
        let until = vcx.update(|_, cx| cx.background_executor().now()) + Duration::from_millis(200);
        vcx.update_window(window, |root, window, cx| {
            scenario::force_frames(root.entity_id(), until, window, cx)
        })
        .unwrap();
        let mut forced = 0;
        loop {
            let now = vcx.update(|_, cx| cx.background_executor().now());
            let before = recorder.frames().len();
            let requested = requested_frames(vcx);
            vcx.run_until_parked();
            if now >= until {
                // The chain's last callback ran and drew nothing.
                assert!(requested <= 1);
                assert_eq!(recorder.frames().len(), before, "no frame after the end");
                break;
            }
            assert_eq!(requested, 1, "only the idle phase's own frame request");
            assert_eq!(recorder.frames().len(), before + 1, "and one frame drawn");
            forced += 1;
            vcx.executor().advance_clock(tokens::FIRST_STEP);
        }
        assert_eq!(forced, 12, "a frame every 16.667 ms for 200 ms");
        for _ in 0..5 {
            vcx.executor().advance_clock(tokens::FIRST_STEP);
            assert_eq!(requested_frames(vcx), 0, "nothing left requesting frames");
        }
    }

    // A probe driver: a box that slides x 0 → 100 on its own track.

    /// The probe's motion: 120 ms in, 90 ms out, by pointer; Reduced fades.
    const PROBE_MOTION: Motion = Motion {
        enter: Duration::from_millis(120),
        exit: Duration::from_millis(90),
        easing: tokens::slide,
        animates: &[Initiator::Pointer],
        reduced: Reduced {
            enter: ReducedPlay::Fade,
            exit: ReducedPlay::Fade,
        },
    };

    struct Probe {
        track: Track,
        /// Every value drawn.
        drawn: Vec<f32>,
    }

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            frames::render_started(window, cx);
            let sample = motion::sample(&mut self.track, window, cx);
            self.drawn.push(sample.value);
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(100. * sample.value))
                        .w(px(20.))
                        .h(px(20.))
                        .opacity(sample.opacity)
                        .bg(rgb(0x3366cc)),
                )
                .child(frames::sentinel())
        }
    }

    struct ProbeHandle(gpui_kit::Entity<Probe>);

    impl Global for ProbeHandle {}

    thread_local! {
        /// Each driver call: opening or not, and the app's policy then.
        static CALLS: RefCell<Vec<(bool, MotionPolicy)>> = const { RefCell::new(Vec::new()) };
    }

    fn probe_toggle(open: bool, cx: &mut App) {
        let policy = motion::policy(cx);
        CALLS.with(|calls| calls.borrow_mut().push((open, policy)));
        let probe = cx.global::<ProbeHandle>().0.clone();
        let now = cx.background_executor().now();
        let to = if open { 1.0 } else { 0.0 };
        probe.update(cx, |p, cx| {
            p.track
                .retarget(to, &PROBE_MOTION, Initiator::Pointer, policy, now);
            cx.notify();
        });
    }

    const PROBES: &[Driver] = &[Driver {
        name: "probe",
        open: |_, cx| probe_toggle(true, cx),
        close: |_, cx| probe_toggle(false, cx),
    }];

    /// One 60 Hz display frame: the clock advances, the frames motion
    /// requested are delivered and whatever they dirtied is drawn.
    fn display_frame(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(tokens::FIRST_STEP);
        requested_frames(cx);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn motion_drivers_run_under_full_and_restore_the_policy(cx: &mut TestAppContext) {
        // A harness's override (any but Full): the drivers must leave it as
        // they found it.
        cx.update(|cx| motion::set_override(Some(MotionPolicy::Reduced), cx));
        let (probe, vcx) = cx.add_window_view(|_, _| Probe {
            track: Track::new(0.0, 1.0),
            drawn: Vec::new(),
        });
        vcx.update(|_, cx| cx.set_global(ProbeHandle(probe.clone())));
        draw(vcx);
        let window: AnyWindowHandle = vcx.update(|window, _| window.window_handle());
        let recorder = vcx.update(FrameRecorder::start);
        let done: Rc<RefCell<Option<anyhow::Result<Vec<DriverRun>>>>> = Rc::default();
        let (out, rec) = (done.clone(), recorder.clone());
        vcx.update(|_, cx| {
            cx.spawn(async move |cx| {
                let runs = scenario::run_drivers(window, &rec, PROBES, 2, cx).await;
                *out.borrow_mut() = Some(runs);
            })
            .detach()
        });
        for _ in 0..1_000 {
            if done.borrow().is_some() {
                break;
            }
            display_frame(vcx);
        }
        let runs = done
            .take()
            .expect("the drivers finished")
            .expect("no error");
        let calls = CALLS.with(|c| c.borrow().clone());
        assert_eq!(
            calls,
            [
                (true, MotionPolicy::Full),
                (false, MotionPolicy::Full),
                (true, MotionPolicy::Full),
                (false, MotionPolicy::Full),
            ],
            "two open-close rounds, each under Full"
        );
        // It moved, as only Full moves it (Reduced has no travel).
        let drawn = probe.read_with(vcx, |p, _| p.drawn.clone());
        assert!(
            drawn.iter().any(|&v| 0.0 < v && v < 1.0),
            "drawn between its ends: {drawn:?}"
        );
        let [run] = &runs[..] else {
            panic!("one run per driver: {}", runs.len())
        };
        assert_eq!(run.name, "probe");
        assert_eq!(run.toggles.len(), 4);
        assert_eq!(run.timeouts, 0);
        for toggle in &run.toggles {
            assert!(
                toggle.frames.len() > 2,
                "a commit frame and animation frames: {}",
                toggle.frames.len()
            );
            assert!(toggle.frames.iter().all(|f| f.at >= toggle.input));
        }
        // Restored: the harness's override, and with it the kit flag.
        vcx.update(|_, cx| {
            assert_eq!(
                cx.try_global::<MotionPolicyOverride>().and_then(|o| o.0),
                Some(MotionPolicy::Reduced)
            );
            assert_eq!(motion::policy(cx), MotionPolicy::Reduced);
            assert!(cx.reduce_motion());
        });
    }

    #[test]
    fn motion_metrics_follow_their_definitions() {
        let t0 = Instant::now();
        let at = |ms: f64| t0 + Duration::from_secs_f64(ms / 1000.0);
        let frame = |ms: f64, draw: f64| Frame {
            draw: Duration::from_secs_f64(draw / 1000.0),
            at: at(ms),
        };
        // Idle: 20 frames at 120 Hz drawing 1 … 20 ms.
        let idle: Vec<Frame> = (0..20)
            .map(|i| frame(1000.0 / 120.0 * f64::from(i), f64::from(i + 1)))
            .collect();
        // Two toggles: the commit frame (not an animation frame), then
        // animation frames; one interval of 16.667 ms is over 1.5 periods
        // (12.5 ms) and one of 12 ms is not.
        let toggles = vec![
            Toggle {
                input: at(100.0),
                frames: vec![
                    frame(106.0, 5.0),
                    frame(114.333, 2.0),
                    frame(131.0, 7.0),
                    frame(143.0, 3.0),
                ],
            },
            Toggle {
                input: at(300.0),
                frames: vec![frame(312.0, 11.0), frame(320.333, 4.0)],
            },
        ];
        let runs = [DriverRun {
            name: "sidebar",
            toggles,
            timeouts: 0,
        }];
        let m = scenario::metrics(&idle, &runs);
        let num = |k: &str| m[k].as_f64().unwrap_or_else(|| panic!("{k}: {:?}", m[k]));
        // Nearest rank: the 19th of 20.
        assert_eq!(num("shell_idle_draw_p95_ms"), 19.0);
        // Animation frames 2, 7, 3, 4: p95 is the 4th of 4.
        assert_eq!(num("sidebar_anim_draw_p95_ms"), 7.0);
        assert_eq!(num("sidebar_anim_draw_max_ms"), 7.0);
        // Commits: 6 and 12 ms after their inputs.
        assert_eq!(num("sidebar_commit_ms"), 12.0);
        assert_eq!(num("sidebar_late_frames"), 1.0);
        // Surfaces without a driver report null.
        for k in [
            "threads_anim_draw_p95_ms",
            "threads_commit_ms",
            "card_anim_draw_max_ms",
            "section_late_frames",
            "accordion_anim_draw_p95_ms",
        ] {
            assert_eq!(m[k], Value::Null, "{k}");
        }
        // No idle frames: no display period, so late frames are unknown.
        let unknown = scenario::metrics(&[], &runs);
        assert_eq!(unknown["shell_idle_draw_p95_ms"], Value::Null);
        assert_eq!(unknown["sidebar_late_frames"], Value::Null);
        assert_eq!(unknown["sidebar_anim_draw_p95_ms"].as_f64(), Some(7.0));
    }

    /// T7.8: the card driver toggles the top card by pointer, and the
    /// review tab registers the running reveal with the window's settling.
    #[gpui_kit::test]
    fn card_driver_collapses_and_expands_the_top_card_by_pointer(cx: &mut TestAppContext) {
        let _sb = Sandbox::isolate();
        let repo = crate::support::code_change_repo();
        let mut shell = shell::start(cx);
        let tab = shell
            .open(shell::commit_req(repo.path(), "refs/tags/head"))
            .expect("open the review");
        shell::draw(shell.cx);
        shell
            .cx
            .update(|_, cx| motion::set_override(Some(MotionPolicy::Full), cx));
        let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
        let top = viewport.read_with(shell.cx, |v, _| v.anchor().file_idx);
        let card = scenario::DRIVERS
            .iter()
            .find(|d| d.name == "card")
            .expect("the card driver");
        let state = |shell: &mut shell::Shell| {
            viewport.read_with(shell.cx, |v, _| {
                (v.document().is_collapsed(top), v.motion_running())
            })
        };
        shell.cx.update(|window, cx| (card.open)(window, cx));
        assert_eq!(state(&mut shell), (true, true), "collapsing by pointer");
        // Registered with the window's settling: a key down settles it.
        shell.cx.simulate_keystrokes("escape");
        assert_eq!(state(&mut shell), (true, false), "settled by a key down");
        shell.cx.update(|window, cx| (card.close)(window, cx));
        assert_eq!(state(&mut shell), (false, true), "expanding by pointer");
    }

    #[test]
    fn motion_metrics_cover_every_registered_budget() {
        // benches/budgets.json registers the M7 app-shell metrics; the app
        // must report each (null until its driver exists), or run-perf
        // rejects the run.
        let budgets: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benches/budgets.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let registered: Vec<&String> = budgets["metrics"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| {
                k.as_str() == "shell_idle_draw_p95_ms"
                    || k.contains("_anim_draw_")
                    || k.ends_with("_late_frames")
                    || k.as_str() == "sidebar_commit_ms"
                    || k.as_str() == "threads_commit_ms"
            })
            .collect();
        assert_eq!(registered.len(), 18, "{registered:?}");
        let reported = scenario::metrics(&[], &[]);
        for k in registered {
            assert!(reported.contains_key(k), "{k} is not reported");
        }
    }
}
