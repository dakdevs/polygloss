//! Motion filmstrips (T7.3, ADR-0030 Verification, plan M7 "Filmstrips").
//!
//! [`filmstrip`] runs one motion under one policy and returns six captures
//! side by side: the commit frame, the next three 60 Hz frames (16.667,
//! 33.333 and 50 ms after it) and ½ and 1 of the motion's duration, stepped
//! by ADR-0030's stepping protocol (the commit frame is t = 0; the first
//! step is exactly `FIRST_STEP`). [`assert_filmstrip`] stacks a Full strip
//! over a Reduced one and compares them with `tests/baselines/
//! e2e-motion-<name>.png` like [`super::screenshot::assert_screenshot`]
//! (`UPDATE_BASELINE=1` rewrites it).
//!
//! The setup (opening windows, loading, drawing until settled) runs under
//! the harness's Off override; `filmstrip` then switches the app's policy to
//! the one under test and puts the setup's back when it is done. Every
//! override keeps `App::reduce_motion` set, so kit motion in the frame (the
//! ⌘O dialog around M13) is settled and the frames are deterministic.
//!
//! **Recordings.** With `POLYGLOSS_MOTION_DUMP=<dir>` (set by
//! `scripts/record-motion.ts`), the strip of the policy named by
//! `POLYGLOSS_MOTION_DUMP_POLICY` (`full`, the default, or `reduced`) also
//! writes a capture every 16.667 ms of executor time, from the commit frame
//! to the first one at or after the end, as `<dir>/frame-0000.png`, …. One
//! strip per policy per test: a second one overwrites the first's frames.
//! The extra draws sample the same clock, so the strip is the same with or
//! without them.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, HeadlessAppContext};
use image::{GenericImage as _, GenericImageView as _, Rgba, RgbaImage};
use polygloss_app::motion::{self, MotionPolicy, MotionPolicyOverride};

use super::screenshot::{self, MAX_DIFFERING_PER_MILLE, Verdict};

/// Frames in a strip.
pub const FRAMES: usize = 6;
/// Pixels between two frames (and between the Full and Reduced rows).
pub const GUTTER: u32 = 8;
/// The gutter's color: a neutral grey that frames light and dark windows.
pub const GUTTER_COLOR: Rgba<u8> = Rgba([128, 128, 128, 255]);
/// The directory a recording's frames go to.
pub const DUMP_ENV: &str = "POLYGLOSS_MOTION_DUMP";
/// Which policy's strip records: `full` (the default) or `reduced`.
pub const DUMP_POLICY_ENV: &str = "POLYGLOSS_MOTION_DUMP_POLICY";

/// `k` display frames of 60 Hz, rounded to the microsecond: 16.667, 33.333,
/// 50 ms, ….
pub fn sixtieths(k: u32) -> Duration {
    Duration::from_micros((u64::from(k) * 1_000_000 + 30) / 60)
}

/// A strip's frame times after the commit: 0, the next three 60 Hz frames,
/// ½ and 1 of `duration` (never earlier than the frame before: a motion
/// shorter than 100 ms repeats its 50 ms frame).
pub fn frame_times(duration: Duration) -> [Duration; FRAMES] {
    let mut times = [
        Duration::ZERO,
        sixtieths(1),
        sixtieths(2),
        sixtieths(3),
        duration / 2,
        duration,
    ];
    for i in 1..FRAMES {
        times[i] = times[i].max(times[i - 1]);
    }
    times
}

/// The recording this strip writes, if any: its directory and frame times.
fn dump(policy: MotionPolicy, duration: Duration) -> Option<(PathBuf, Vec<Duration>)> {
    let dir = PathBuf::from(std::env::var_os(DUMP_ENV)?);
    let wanted = std::env::var(DUMP_POLICY_ENV).unwrap_or_else(|_| "full".into());
    if !format!("{policy:?}").eq_ignore_ascii_case(&wanted) {
        return None;
    }
    std::fs::create_dir_all(&dir).expect("create the recording's directory");
    let mut times = Vec::new();
    for k in 0.. {
        let t = sixtieths(k);
        times.push(t);
        if t >= duration {
            break;
        }
    }
    Some((dir, times))
}

/// Delivers the frames motion requested, draws and captures `window`.
fn frame(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> RgbaImage {
    cx.update_window(window, |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .expect("the window is open");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("the window is open");
    cx.capture_screenshot(window)
        .expect("render the frame with Metal")
}

/// Runs the motion `start` commits under `policy` and returns its six
/// frames side by side (see the module docs); `duration` is the motion's
/// under that policy (a Reduced fade's, for a Reduced strip).
pub fn filmstrip(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    policy: MotionPolicy,
    duration: Duration,
    start: impl FnOnce(&mut HeadlessAppContext),
) -> RgbaImage {
    let setup = cx.update(|cx| cx.try_global::<MotionPolicyOverride>().and_then(|o| o.0));
    cx.update(|cx| motion::set_override(Some(policy), cx));
    start(cx);
    let film = frame_times(duration);
    let dump = dump(policy, duration);
    let mut times: Vec<Duration> = film.to_vec();
    if let Some((_, extra)) = &dump {
        times.extend(extra);
    }
    times.sort();
    times.dedup();
    let mut frames = Vec::with_capacity(FRAMES);
    let mut now = Duration::ZERO;
    for t in times {
        cx.advance_clock(t - now);
        now = t;
        let image = frame(cx, window);
        if let Some((dir, extra)) = &dump
            && let Some(k) = extra.iter().position(|&e| e == t)
        {
            let path = dir.join(format!("frame-{k:04}.png"));
            image
                .save(&path)
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        }
        for _ in film.iter().filter(|&&f| f == t) {
            frames.push(image.clone());
        }
    }
    cx.update(|cx| motion::set_override(setup, cx));
    side_by_side(&frames)
}

/// `frames` in a row, [`GUTTER`] apart.
fn side_by_side(frames: &[RgbaImage]) -> RgbaImage {
    let (w, h) = frames[0].dimensions();
    let n = frames.len() as u32;
    let mut strip = RgbaImage::from_pixel(n * w + (n - 1) * GUTTER, h, GUTTER_COLOR);
    for (i, f) in frames.iter().enumerate() {
        strip
            .copy_from(f, i as u32 * (w + GUTTER), 0)
            .expect("every frame is the window's size");
    }
    strip
}

/// Frame `index` of a one-row strip.
pub fn frame_of(strip: &RgbaImage, index: usize) -> RgbaImage {
    let w = (strip.width() - (FRAMES as u32 - 1) * GUTTER) / FRAMES as u32;
    strip
        .view(index as u32 * (w + GUTTER), 0, w, strip.height())
        .to_image()
}

/// Checks the Full strip over the Reduced one against
/// `tests/baselines/e2e-motion-<name>.png`; `UPDATE_BASELINE=1` rewrites
/// it. Panics on a mismatch, naming the `.actual.png` and `.diff.png`.
pub fn assert_filmstrip(name: &str, full: RgbaImage, reduced: RgbaImage) {
    assert_eq!(
        full.width(),
        reduced.width(),
        "{name}: the Full and Reduced strips are one window's frames"
    );
    let mut both = RgbaImage::from_pixel(
        full.width(),
        full.height() + GUTTER + reduced.height(),
        GUTTER_COLOR,
    );
    both.copy_from(&full, 0, 0).expect("fits");
    both.copy_from(&reduced, 0, full.height() + GUTTER)
        .expect("fits");
    let test = format!("e2e_motion_{name}");
    let update = std::env::var_os("UPDATE_BASELINE").is_some_and(|v| v == "1");
    let dir = screenshot::baselines_dir();
    std::fs::create_dir_all(&dir).expect("create tests/baselines");
    let verdict = screenshot::check(&dir, &test, &both, update).expect("read or write the strip");
    let stem = dir.join(screenshot::baseline_name(&test));
    match verdict {
        Verdict::Matched { .. } => {}
        Verdict::Updated => eprintln!("{test}: wrote {}.png", stem.display()),
        Verdict::Missing => panic!(
            "{test}: no baseline {}.png; the strip is in {0}.actual.png. \
             Look at every frame, then record it with UPDATE_BASELINE=1.",
            stem.display()
        ),
        Verdict::SizeMismatch { actual, expected } => panic!(
            "{test}: strip is {actual:?} px, baseline {expected:?} px; see {}.actual.png",
            stem.display()
        ),
        Verdict::Mismatch { differing, total } => panic!(
            "{test}: {differing} of {total} pixels differ (more than {MAX_DIFFERING_PER_MILLE}‰); \
             see {0}.actual.png and {0}.diff.png, or rerun with UPDATE_BASELINE=1 \
             if the change is intended",
            stem.display()
        ),
    }
}
