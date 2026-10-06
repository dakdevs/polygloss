//! The filmstrip helper (T7.3, ADR-0030 Verification): six frames of a
//! motion stepped by the stepping protocol, drawn by Metal, under the policy
//! a strip asks for while the kit's motion stays settled.
//!
//! - `filmstrip_captures_six_frames`: a probe moving linearly on the
//!   executor clock sits where the frame times put it in each crop.
//! - `filmstrip_keeps_kit_motion_settled`: a kit dialog opened in the setup
//!   is fully in on frame 0 of a Full and of a Reduced strip.
//! - `e2e_motion_probe`: the probe's filmstrip baseline (Full over Reduced),
//!   the helper's own example and `scripts/record-motion.ts`'s smallest
//!   target.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Context, Entity, HeadlessAppContext, IntoElement,
    ParentElement as _, Render, Styled as _, Window, div, point, px, rgb, size, white,
};
use polygloss_app::motion::{self, MotionPolicy, MotionPolicyOverride, tokens};

use crate::support::Sandbox;
use crate::support::filmstrip::{self, assert_filmstrip, filmstrip, frame_of};
use crate::support::harness::Test;
use crate::support::ink::Ink;
use crate::support::screenshot;

pub const TESTS: &[Test] = &crate::tests![
    filmstrip_captures_six_frames,
    filmstrip_keeps_kit_motion_settled,
    e2e_motion_probe,
];

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// The probe's window, in points.
const PROBE_W: f32 = 240.0;
const PROBE_H: f32 = 40.0;
/// Where the probe's square starts.
const PROBE_X: f32 = 20.0;

/// A 16 pt square that, once committed, moves `x = 100 · t / 240 ms`
/// linearly on the executor clock (Full), or stays at its end and fades in
/// over QUICK (Reduced): the probe's own reading of the policy, so the
/// strips show which policy each row ran under.
struct Probe {
    start: Option<Instant>,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (travel, opacity) = match self.start {
            None => (0.0, 1.0),
            Some(start) => {
                let t = cx.background_executor().now() - start;
                let share = |of: Duration| (t.as_secs_f32() / of.as_secs_f32()).min(1.0);
                if motion::policy(cx) == MotionPolicy::Reduced {
                    (100.0, share(tokens::QUICK))
                } else {
                    (100.0 * share(ms(240)), 1.0)
                }
            }
        };
        let x = motion::quantize(PROBE_X + travel, window.scale_factor());
        div().size_full().bg(white()).child(
            div()
                .absolute()
                .left(px(x))
                .top(px(12.))
                .w(px(16.))
                .h(px(16.))
                .opacity(opacity)
                .bg(rgb(0x3366cc)),
        )
    }
}

fn open_probe(cx: &mut HeadlessAppContext) -> (AnyWindowHandle, Entity<Probe>) {
    let window = cx
        .open_window(size(px(PROBE_W), px(PROBE_H)), |_, cx| {
            cx.new(|_| Probe { start: None })
        })
        .expect("open the probe's window");
    let probe = window
        .update(cx, |_, _, cx| cx.entity())
        .expect("the window is open");
    screenshot::park_pointer(cx, *window);
    screenshot::draw(cx, *window);
    (*window, probe)
}

/// The commit: the probe starts moving now.
fn commit(cx: &mut HeadlessAppContext, probe: &Entity<Probe>) {
    probe.update(cx, |p, cx| {
        p.start = Some(cx.background_executor().now());
        cx.notify();
    });
}

fn override_policy(cx: &mut HeadlessAppContext) -> Option<MotionPolicy> {
    cx.update(|cx| cx.try_global::<MotionPolicyOverride>().and_then(|o| o.0))
}

fn filmstrip_captures_six_frames() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app();
    let (window, probe) = open_probe(&mut cx);
    let strip = filmstrip(&mut cx, window, MotionPolicy::Full, ms(240), |cx| {
        commit(cx, &probe)
    });
    let (w, h) = (
        PROBE_W as u32 * screenshot::SCALE,
        PROBE_H as u32 * screenshot::SCALE,
    );
    assert_eq!(
        strip.dimensions(),
        (6 * w + 5 * filmstrip::GUTTER, h),
        "six frames side by side"
    );
    // The commit frame, the next three 60 Hz frames, ½ and 1 of 240 ms.
    let expected = [0.0, 6.9, 13.9, 20.8, 50.0, 100.0];
    let band = Bounds::new(point(0, 12), size(PROBE_W as u32, 16));
    for (i, want) in expected.into_iter().enumerate() {
        let x = Ink::default()
            .first_x(&frame_of(&strip, i), band)
            .unwrap_or_else(|| panic!("frame {i} shows the probe"))
            - PROBE_X;
        assert!((x - want).abs() <= 0.5, "frame {i}: x {x}, expected {want}");
    }
    // The setup's policy is back for whatever the test draws next.
    assert_eq!(override_policy(&mut cx), Some(MotionPolicy::Off));
}

/// A plain view under the kit's root, which hosts dialogs.
struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(white())
    }
}

fn filmstrip_keeps_kit_motion_settled() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    cx.update(gpui_kit::init);
    cx.update(|cx| motion::set_override(Some(MotionPolicy::Off), cx));
    for policy in [MotionPolicy::Full, MotionPolicy::Reduced] {
        // The setup, under Off: a fresh window (fresh kit element state) with
        // a dialog drawn once. The kit's entrance runs on the wall clock
        // (250 ms), so a frame drawn just after this one with the kit's
        // reduce-motion flag cleared would catch it mid-entrance.
        let window = *cx
            .open_window(size(px(480.), px(320.)), |window, cx| {
                let blank = cx.new(|_| Blank);
                cx.new(|cx| Root::new(blank, window, cx))
            })
            .expect("open the kit's window");
        screenshot::park_pointer(&mut cx, window);
        cx.update_window(window, |_, window, cx| {
            window.open_dialog(cx, |dialog, _, _| {
                dialog
                    .title("A kit dialog")
                    .child("Its entrance runs on the wall clock.")
            })
        })
        .expect("the window is open");
        screenshot::draw(&mut cx, window);
        let strip = filmstrip(&mut cx, window, policy, ms(240), |_| {});
        // Well past the entrance on the wall clock: the dialog fully in.
        std::thread::sleep(ms(300));
        screenshot::draw(&mut cx, window);
        let settled = cx.capture_screenshot(window).expect("capture");
        for i in 0..6 {
            let cmp = screenshot::compare(&frame_of(&strip, i), &settled)
                .expect("frames are the window's size");
            assert!(
                cmp.within_budget(),
                "{policy:?} frame {i}: {} of {} pixels differ from the settled dialog",
                cmp.differing,
                cmp.total
            );
        }
    }
}

fn e2e_motion_probe() {
    let _sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app();
    let (window, probe) = open_probe(&mut cx);
    let full = filmstrip(&mut cx, window, MotionPolicy::Full, ms(240), |cx| {
        commit(cx, &probe)
    });
    // Back to rest under the setup's Off, then the Reduced row: a fade over
    // QUICK in place.
    probe.update(&mut cx, |p, _| p.start = None);
    screenshot::draw(&mut cx, window);
    let reduced = filmstrip(
        &mut cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        |cx| commit(cx, &probe),
    );
    assert_filmstrip("probe", full, reduced);
}
