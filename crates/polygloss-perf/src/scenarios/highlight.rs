//! `highlight`: `highlight_ms`, the time from the last scroll event to the
//! first frame in which every visible row carries its tokens, p95 over 20
//! stops.
//!
//! Each stop jumps to a seeded random offset in the document (so every stop
//! lands somewhere new), waits there until the jump's rows are highlighted
//! (reported, not budgeted: `highlight_after_jump_p95_ms`, the cold case),
//! scrolls 1 s at 4,000 px/s from there (down, or up when the document ends
//! first) and stops. The time runs from that last
//! scroll step to the first frame after it with no unhighlighted rows (rows
//! of sides that never get tokens, e.g. over 100k lines or without a
//! grammar, do not count; loading rows do). A stop that takes longer than
//! 10 s counts as 10 s and as a timeout.

use std::time::{Duration, Instant};

use gpui_kit::AsyncApp;
use serde_json::json;

use super::Rng;
use crate::args::Knobs;
use crate::harness::{Harness, within};
use crate::metrics::{ScenarioResult, Series, ms};

/// How long each stop's scroll lasts.
const FLING: Duration = Duration::from_secs(1);
/// Longest a stop may wait for tokens.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// No frame for this long between stops counts as idle.
const QUIET: Duration = Duration::from_millis(200);

/// A seeded random scroll offset in `[0, max_scroll]`.
pub fn stop_target(rng: &mut Rng, max_scroll: f64) -> f64 {
    if max_scroll <= 0.0 {
        0.0
    } else {
        rng.unit() * max_scroll
    }
}

/// Down at `speed` when `secs` of it fit below `start`, else up when they fit
/// above it, else down (the scroll then turns at the end).
pub fn fling_velocity(start: f64, max_scroll: f64, speed: f32, secs: f32) -> f32 {
    let travel = f64::from(speed * secs);
    if start + travel <= max_scroll || start - travel < 0.0 {
        speed
    } else {
        -speed
    }
}

pub async fn run(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    rng: &mut Rng,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let mut stops = Vec::new();
    let mut jumps = Vec::new();
    let mut offsets = Vec::new();
    let mut timeouts = 0u32;
    for _ in 0..knobs.stops {
        let (top, max) = h.read(cx, |v| {
            let doc = v.document();
            (doc.scroll_top(), doc.max_scroll())
        });
        let target = stop_target(rng, max);
        offsets.push(target.round());
        let jumped = Instant::now();
        h.update(cx, |v, cx| v.scroll_by((target - top) as f32, cx));
        // Informational: how long a jump into cold content takes to show
        // highlighted (nothing near the target was loaded before it).
        let after_jump = h
            .wait_for(cx, STOP_TIMEOUT, |f| {
                f.at > jumped && f.stats.unhighlighted_rows == 0
            })
            .await?;
        jumps.push(after_jump.map_or(ms(STOP_TIMEOUT), |f| ms(f.at - jumped)));
        let velocity = fling_velocity(target, max, knobs.speed, FLING.as_secs_f32());
        let done = h.scroll(cx, velocity, FLING)?;
        let last = within(
            cx,
            FLING + Duration::from_secs(10),
            done,
            "a highlight stop",
        )
        .await?;
        let stopped = last.unwrap_or(jumped);
        let highlighted = h
            .wait_for(cx, STOP_TIMEOUT, |f| {
                f.at > stopped && f.stats.unhighlighted_rows == 0
            })
            .await?;
        match highlighted {
            Some(frame) => stops.push(ms(frame.at - stopped)),
            None => {
                timeouts += 1;
                stops.push(ms(STOP_TIMEOUT));
            }
        }
        h.quiet(cx, QUIET, STOP_TIMEOUT).await?;
    }
    let series = Series::new(&stops);
    result.metric("highlight_ms", series.p95());
    result.metric("highlight_max_ms", series.max());
    result.metric("highlight_timeouts", Some(f64::from(timeouts)));
    result.metric("highlight_after_jump_p95_ms", Series::new(&jumps).p95());
    result.info(
        "highlight",
        json!({ "fling_ms": ms(FLING), "speed_px_s": knobs.speed, "stop_offsets": offsets }),
    );
    result.samples("stop_ms", stops);
    result.samples("jump_ms", jumps);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::Rng;

    #[test]
    fn stops_land_at_seeded_offsets_inside_the_document() {
        let mut a = Rng::new(3);
        let mut b = Rng::new(3);
        for _ in 0..100 {
            let y = stop_target(&mut a, 25_000.0);
            assert!((0.0..=25_000.0).contains(&y), "{y}");
            assert_eq!(y, stop_target(&mut b, 25_000.0));
        }
        assert_eq!(stop_target(&mut a, 0.0), 0.0);
    }

    #[test]
    fn flings_go_down_unless_the_document_ends_first() {
        // 1 s at 4,000 px/s needs 4,000 px below the stop's start.
        assert_eq!(fling_velocity(0.0, 10_000.0, 4_000.0, 1.0), 4_000.0);
        assert_eq!(fling_velocity(6_000.0, 10_000.0, 4_000.0, 1.0), 4_000.0);
        assert_eq!(fling_velocity(6_001.0, 10_000.0, 4_000.0, 1.0), -4_000.0);
        // A short document bounces instead (the scroller turns at the ends).
        assert_eq!(fling_velocity(0.0, 1_000.0, 4_000.0, 1.0), 4_000.0);
    }
}
