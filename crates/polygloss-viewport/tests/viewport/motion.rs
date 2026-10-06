//! The motion core (ADR-0030, T7.2): tokens, the policy, `play` and the
//! `Track` sampler. Expected values are this file's own: a bisection solver
//! for the cubic Béziers and hand-computed durations, never the production
//! easing.

use std::time::{Duration, Instant};

use gpui_kit::TestAppContext;
use polygloss_viewport::motion::{
    Initiator, Motion, MotionPolicy, MotionPolicyOverride, Play, Reduced, ReducedPlay, Track, play,
    policy, quantize, tokens,
};

/// `cubic-bezier(x1, y1, x2, y2)` at `t` by bisection on x (the test's
/// oracle; production uses gpui-base's solver).
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let at = |a: f64, b: f64, s: f64| {
        3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..200 {
        let mid = (low + high) / 2.0;
        if at(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    at(y1, y2, (low + high) / 2.0)
}

/// SLIDE, `cubic_bezier(0.25, 1, 0.5, 1)`.
fn slide(t: f64) -> f64 {
    bezier(0.25, 1.0, 0.5, 1.0, t)
}

fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

fn close(actual: f32, expected: f64, what: &str) {
    assert!(
        (f64::from(actual) - expected).abs() < 1e-3,
        "{what}: {actual} ≠ {expected}"
    );
}

/// A panel-like motion: 240 ms in, 180 ms out, SLIDE, pointer only.
fn panel() -> Motion {
    Motion {
        enter: ms(240.0),
        exit: ms(180.0),
        easing: tokens::slide,
        animates: &[Initiator::Pointer],
        reduced: Reduced {
            enter: ReducedPlay::Fade,
            exit: ReducedPlay::Fade,
        },
    }
}

#[test]
fn exit_durations_match_adr_0030() {
    for (entry, exit) in [
        (240, 180),
        (200, 150),
        (190, 140),
        (180, 140),
        (150, 110),
        (100, 100),
    ] {
        assert_eq!(
            tokens::exit(Duration::from_millis(entry)),
            Duration::from_millis(exit),
            "exit({entry} ms)"
        );
    }
}

#[test]
fn reveal_duration_follows_distance() {
    for (delta, viewport, expected) in [
        (0.0, 900.0, 150),
        (100.0, 900.0, 160),
        (400.0, 900.0, 190),
        (-400.0, 900.0, 190),
        (900.0, 900.0, 200),
        (2_000.0, 400.0, 190),
    ] {
        assert_eq!(
            tokens::reveal(delta, viewport),
            Duration::from_millis(expected),
            "reveal({delta} pt in {viewport} pt)"
        );
    }
}

#[test]
fn play_follows_policy_initiator_and_direction() {
    use Initiator::{Keyboard, Pointer, Programmatic};
    use MotionPolicy::{Full, Off, Reduced as R};
    use ReducedPlay::{Fade, Snap};
    let motion = |animates: &'static [Initiator], enter: ReducedPlay, exit: ReducedPlay| Motion {
        enter: ms(150.0),
        exit: ms(110.0),
        easing: tokens::out,
        animates,
        reduced: Reduced { enter, exit },
    };
    const POINTER: &[Initiator] = &[Initiator::Pointer];
    const POINTER_KEYS: &[Initiator] = &[Initiator::Pointer, Initiator::Keyboard];
    // (animates, reduced enter, reduced exit, entering, initiator, policy) → play
    let table = [
        (POINTER, Fade, Fade, true, Pointer, Full, Play::Animate),
        (POINTER, Fade, Fade, false, Pointer, Full, Play::Animate),
        (POINTER, Fade, Fade, true, Keyboard, Full, Play::Snap),
        (POINTER, Fade, Fade, true, Programmatic, Full, Play::Snap),
        (
            POINTER_KEYS,
            Fade,
            Fade,
            true,
            Keyboard,
            Full,
            Play::Animate,
        ),
        (
            POINTER_KEYS,
            Fade,
            Fade,
            true,
            Programmatic,
            Full,
            Play::Snap,
        ),
        (POINTER, Fade, Fade, true, Pointer, R, Play::Fade),
        (POINTER, Fade, Fade, false, Pointer, R, Play::Fade),
        (POINTER, Fade, Snap, true, Pointer, R, Play::Fade),
        (POINTER, Fade, Snap, false, Pointer, R, Play::Snap),
        (POINTER, Snap, Fade, true, Pointer, R, Play::Snap),
        (POINTER, Snap, Fade, false, Pointer, R, Play::Fade),
        (POINTER, Fade, Fade, true, Keyboard, R, Play::Snap),
        (POINTER, Fade, Fade, true, Programmatic, R, Play::Snap),
        (POINTER, Fade, Fade, true, Pointer, Off, Play::Snap),
        (POINTER, Fade, Fade, false, Pointer, Off, Play::Snap),
        (POINTER_KEYS, Fade, Fade, true, Keyboard, Off, Play::Snap),
    ];
    for (animates, enter, exit, entering, initiator, policy, expected) in table {
        assert_eq!(
            play(&motion(animates, enter, exit), entering, initiator, policy),
            expected,
            "{animates:?} reduced {enter:?}/{exit:?}, entering {entering}, {initiator:?}, {policy:?}"
        );
    }
}

#[gpui_kit::test]
fn policy_is_derived_never_stored(cx: &mut TestAppContext) {
    assert_eq!(
        cx.update(|cx| policy(cx)),
        MotionPolicy::Full,
        "the flag false"
    );
    cx.update(|cx| cx.set_reduce_motion(true));
    assert_eq!(
        cx.update(|cx| policy(cx)),
        MotionPolicy::Reduced,
        "the flag true"
    );
    cx.update(|cx| cx.set_global(MotionPolicyOverride(Some(MotionPolicy::Full))));
    assert_eq!(
        cx.update(|cx| policy(cx)),
        MotionPolicy::Full,
        "the override wins"
    );
    cx.update(|cx| cx.set_global(MotionPolicyOverride(None)));
    assert_eq!(
        cx.update(|cx| policy(cx)),
        MotionPolicy::Reduced,
        "no override: the flag"
    );
    cx.update(|cx| cx.set_reduce_motion(false));
    assert_eq!(cx.update(|cx| policy(cx)), MotionPolicy::Full);
}

#[test]
fn track_samples_full_travel_then_between_then_settled() {
    let t0 = Instant::now();
    let mut track = Track::new(0.0, 12.0);
    let motion = Motion {
        enter: ms(240.0),
        ..panel()
    };
    track.retarget(12.0, &motion, Initiator::Pointer, MotionPolicy::Full, t0);
    let commit = track.sample(t0);
    assert_eq!(
        (commit.value, commit.settled),
        (0.0, false),
        "the commit frame"
    );
    assert_eq!(commit.opacity, 1.0);
    close(
        track.sample(t0 + tokens::FIRST_STEP).value,
        12.0 * slide(16.667 / 240.0),
        "the first step",
    );
    let half = track.sample(t0 + ms(120.0));
    close(half.value, 12.0 * slide(0.5), "half way");
    assert!(0.0 < half.value && half.value < 12.0 && !half.settled);
    let end = track.sample(t0 + ms(240.0));
    assert_eq!(
        (end.value, end.opacity, end.settled),
        (12.0, 1.0, true),
        "settled"
    );
    assert!(track.is_settled());
}

#[test]
fn track_clamps_its_first_step() {
    let t0 = Instant::now();
    let mut track = Track::new(0.0, 12.0);
    track.retarget(12.0, &panel(), Initiator::Pointer, MotionPolicy::Full, t0);
    // A heavy commit: the first sample comes 100 ms after the retarget.
    close(
        track.sample(t0 + ms(100.0)).value,
        12.0 * slide(16.667 / 240.0),
        "the first step reads as 16.667 ms",
    );
    // Later steps are not clamped: 50 ms after that, 66.667 ms in.
    close(
        track.sample(t0 + ms(150.0)).value,
        12.0 * slide(66.667 / 240.0),
        "the next step",
    );
}

#[test]
fn retarget_continues_from_the_sampled_value() {
    let t0 = Instant::now();
    let mut track = Track::new(0.0, 1.0);
    track.retarget(1.0, &panel(), Initiator::Pointer, MotionPolicy::Full, t0);
    track.sample(t0);
    track.sample(t0 + tokens::FIRST_STEP);
    let at = t0 + ms(60.0);
    let before = track.sample(at).value;
    // Toward a third value: from where it is, never from 0.
    track.retarget(0.5, &panel(), Initiator::Pointer, MotionPolicy::Full, at);
    let after = track.sample(at);
    assert_eq!(after.value, before, "continuous at the retarget");
    assert!(!after.settled);
    let next = track.sample(at + tokens::FIRST_STEP).value;
    assert!(0.5 < next && next < before, "moving toward 0.5: {next}");
}

#[test]
fn reversal_takes_the_new_directions_duration_times_the_share_travelled() {
    // An open over 240 ms reversed at 120 ms: the close lasts 180 · SLIDE(½).
    let t0 = Instant::now();
    let mut track = Track::new(0.0, 1.0);
    track.retarget(1.0, &panel(), Initiator::Pointer, MotionPolicy::Full, t0);
    track.sample(t0);
    track.sample(t0 + tokens::FIRST_STEP);
    let reversed = t0 + ms(120.0);
    let from = track.sample(reversed).value;
    close(from, slide(0.5), "half open");
    track.retarget(
        0.0,
        &panel(),
        Initiator::Pointer,
        MotionPolicy::Full,
        reversed,
    );
    let close_ms = 180.0 * slide(0.5);
    track.sample(reversed);
    track.sample(reversed + tokens::FIRST_STEP);
    close(
        track.sample(reversed + ms(close_ms / 2.0)).value,
        f64::from(from) * (1.0 - slide(0.5)),
        "half way through the shortened close",
    );
    assert!(!track.sample(reversed + ms(close_ms - 1.0)).settled);
    assert!(track.sample(reversed + ms(close_ms + 0.01)).settled);

    // A close over 180 ms reversed at 90 ms: the open lasts 240 · SLIDE(½).
    let t0 = reversed + ms(500.0);
    let mut track = Track::new(1.0, 1.0);
    track.retarget(0.0, &panel(), Initiator::Pointer, MotionPolicy::Full, t0);
    track.sample(t0);
    track.sample(t0 + tokens::FIRST_STEP);
    let reversed = t0 + ms(90.0);
    let from = track.sample(reversed).value;
    close(from, 1.0 - slide(0.5), "half closed");
    track.retarget(
        1.0,
        &panel(),
        Initiator::Pointer,
        MotionPolicy::Full,
        reversed,
    );
    let open_ms = 240.0 * slide(0.5);
    track.sample(reversed);
    track.sample(reversed + tokens::FIRST_STEP);
    close(
        track.sample(reversed + ms(open_ms / 2.0)).value,
        f64::from(from) + (1.0 - f64::from(from)) * slide(0.5),
        "half way through the shortened open",
    );
    assert!(!track.sample(reversed + ms(open_ms - 1.0)).settled);
    assert!(track.sample(reversed + ms(open_ms + 0.01)).settled);
}

#[test]
fn a_frozen_track_holds_its_value() {
    let t0 = Instant::now();
    let mut track = Track::new(0.0, 1.0);
    track.retarget(1.0, &panel(), Initiator::Pointer, MotionPolicy::Full, t0);
    track.sample(t0);
    track.sample(t0 + tokens::FIRST_STEP);
    track.freeze(t0 + ms(60.0));
    assert!(track.is_frozen());
    let held = track.sample(t0 + ms(60.0));
    close(held.value, slide(60.0 / 240.0), "frozen at 60 ms");
    assert_eq!(track.sample(t0 + ms(80.0)), held, "80 ms");
    assert_eq!(track.sample(t0 + ms(120.0)), held, "120 ms");
    assert_eq!(track.sample(t0 + ms(400.0)), held, "past the duration");
    assert!(!track.is_settled());
    track.settle();
    assert!(!track.is_frozen());
    let settled = track.sample(t0 + ms(400.0));
    assert_eq!(
        (settled.value, settled.settled),
        (1.0, true),
        "settled after"
    );
}

#[test]
fn quantize_rounds_to_device_pixels() {
    assert_eq!(quantize(3.3, 2.0), 3.5);
    assert_eq!(quantize(3.2, 2.0), 3.0);
    assert_eq!(quantize(3.3, 1.0), 3.0);
    assert_eq!(quantize(-3.3, 2.0), -3.5);
}
