//! `scroll`: `scroll_p95_ms`, the p95 of per-frame prepaint + paint CPU time
//! over 10 s of scripted scrolling at 4,000 px/s plus 20 random file jumps
//! (OQ-P6), with frame-to-frame intervals (p95 and max) next to it and the
//! frames over 16.7 ms counted.
//!
//! The scroll starts at the settled top of the document, one step per
//! display frame, turning around at the document's ends; a jump to a random
//! file (seeded) happens in the middle of every twentieth of the run and the
//! scroll carries on from there.

use std::time::{Duration, Instant};

use gpui_kit::AsyncApp;
use serde_json::json;

use super::Rng;
use crate::args::Knobs;
use crate::harness::{Harness, sleep_until, within};
use crate::metrics::{SLOW_FRAME_MS, ScenarioResult, Series, intervals_ms, ms};

/// A scroll within this many px of an end turns around.
const EDGE: f64 = 0.5;

/// A jump to `file`'s header, `at` into the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jump {
    pub at: Duration,
    pub file: u32,
}

/// `n` jumps to seeded random files, one in the middle of each `n`-th of
/// `duration`.
pub fn plan_jumps(files: u32, duration: Duration, n: u32, rng: &mut Rng) -> Vec<Jump> {
    if files == 0 || n == 0 {
        return Vec::new();
    }
    let nanos = duration.as_nanos();
    (0..n)
        .map(|k| Jump {
            at: Duration::from_nanos((nanos * u128::from(2 * k + 1) / u128::from(2 * n)) as u64),
            file: rng.below(u64::from(files)) as u32,
        })
        .collect()
}

/// Scripted scrolling: how far to move on each frame.
#[derive(Debug, Clone)]
pub struct Scroller {
    /// px/s; negative scrolls up.
    velocity: f32,
    last: Instant,
    until: Instant,
    last_step: Option<Instant>,
}

impl Scroller {
    pub fn new(velocity: f32, start: Instant, until: Instant) -> Scroller {
        Scroller {
            velocity,
            last: start,
            until,
            last_step: None,
        }
    }

    /// The move for a frame at `now` (`velocity × time since the last
    /// step`), turning around at the document's ends; `None` once the scroll
    /// is over. With nothing to scroll it moves 0 (the frame is still drawn).
    pub fn step(&mut self, now: Instant, scroll_top: f64, max_scroll: f64) -> Option<f32> {
        if now >= self.until {
            return None;
        }
        let dt = now.saturating_duration_since(self.last).as_secs_f32();
        self.last = now;
        self.last_step = Some(now);
        if max_scroll <= 0.0 {
            return Some(0.0);
        }
        if (self.velocity > 0.0 && scroll_top >= max_scroll - EDGE)
            || (self.velocity < 0.0 && scroll_top <= EDGE)
        {
            self.velocity = -self.velocity;
        }
        Some(self.velocity * dt)
    }

    /// When the scroll last moved (the last scroll event).
    pub fn last_step(&self) -> Option<Instant> {
        self.last_step
    }
}

pub async fn run(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    rng: &mut Rng,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let files: Vec<u32> = (0..h.read(cx, |v| v.document().len())).collect();
    measure(h, cx, knobs, rng, result, "", &files).await
}

/// The scroll run, its metrics named `<prefix>scroll_p95_ms` and so on, its
/// jumps going to seeded random files of `targets`.
pub async fn measure(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    rng: &mut Rng,
    result: &mut ScenarioResult,
    prefix: &str,
    targets: &[u32],
) -> anyhow::Result<()> {
    let mut jumps = plan_jumps(targets.len() as u32, knobs.scroll, knobs.jumps, rng);
    for jump in &mut jumps {
        jump.file = targets[jump.file as usize];
    }
    h.drain();
    let t0 = Instant::now();
    let done = h.scroll(cx, knobs.speed, knobs.scroll)?;
    for jump in &jumps {
        sleep_until(cx, t0 + jump.at).await;
        h.jump_to_file(cx, jump.file);
    }
    within(
        cx,
        knobs.scroll + Duration::from_secs(10),
        done,
        "scrolling",
    )
    .await?;
    let end = t0 + knobs.scroll;
    let frames: Vec<_> = h
        .drain()
        .into_iter()
        .filter(|f| f.at >= t0 && f.at <= end)
        .collect();
    anyhow::ensure!(!frames.is_empty(), "no frames were painted while scrolling");
    let cpu: Vec<f64> = frames.iter().map(|f| ms(f.cpu())).collect();
    let times: Vec<Instant> = frames.iter().map(|f| f.at).collect();
    let gaps = intervals_ms(&times);
    let (cpu_s, gaps_s) = (Series::new(&cpu), Series::new(&gaps));
    let mut metric = |name: &str, value| result.metric(&format!("{prefix}{name}"), value);
    metric("scroll_p95_ms", cpu_s.p95());
    metric("scroll_p99_ms", cpu_s.p99());
    metric("scroll_max_ms", cpu_s.max());
    metric("frame_interval_p95_ms", gaps_s.p95());
    metric("frame_interval_max_ms", gaps_s.max());
    metric("frames_over_16_7ms", Some(cpu_s.over(SLOW_FRAME_MS) as f64));
    metric(
        "intervals_over_16_7ms",
        Some(gaps_s.over(SLOW_FRAME_MS) as f64),
    );
    metric("frames", Some(cpu_s.len() as f64));
    result.info(
        &format!("{prefix}scroll"),
        json!({
            "duration_ms": ms(knobs.scroll),
            "speed_px_s": knobs.speed,
            "jumps": jumps.iter().map(|j| json!({ "at_ms": ms(j.at), "file": j.file })).collect::<Vec<_>>(),
            "shaped_lines": frames.iter().map(|f| u64::from(f.stats.shaped_lines)).sum::<u64>(),
            "unhighlighted_frames": frames.iter().filter(|f| f.stats.unhighlighted_rows > 0).count(),
            "loading_frames": frames.iter().filter(|f| f.stats.loading_rows > 0).count(),
        }),
    );
    result.samples(&format!("{prefix}frame_ms"), cpu);
    result.samples(&format!("{prefix}interval_ms"), gaps);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::scenarios::Rng;

    #[test]
    fn jumps_are_spread_evenly_and_seeded() {
        let jumps = plan_jumps(100, Duration::from_secs(10), 20, &mut Rng::new(1));
        assert_eq!(jumps.len(), 20);
        // In the middle of each twentieth of the run: 0.25 s, 0.75 s, … 9.75 s.
        assert_eq!(jumps[0].at, Duration::from_millis(250));
        assert_eq!(jumps[1].at, Duration::from_millis(750));
        assert_eq!(jumps[19].at, Duration::from_millis(9_750));
        assert!(jumps.iter().all(|j| j.file < 100));
        assert_eq!(
            jumps,
            plan_jumps(100, Duration::from_secs(10), 20, &mut Rng::new(1))
        );
        assert_ne!(
            jumps,
            plan_jumps(100, Duration::from_secs(10), 20, &mut Rng::new(2))
        );
        // A one-file corpus (huge-file) jumps to its only file.
        let one = plan_jumps(1, Duration::from_secs(10), 20, &mut Rng::new(1));
        assert!(one.iter().all(|j| j.file == 0));
        assert!(plan_jumps(0, Duration::from_secs(10), 20, &mut Rng::new(1)).is_empty());
    }

    #[test]
    fn scroller_moves_at_its_speed() {
        let t0 = Instant::now();
        let mut s = Scroller::new(4_000.0, t0, t0 + Duration::from_secs(10));
        let dy = s.step(t0 + Duration::from_millis(10), 0.0, 1e6).unwrap();
        assert!((dy - 40.0).abs() < 1e-3, "{dy}");
        let dy = s.step(t0 + Duration::from_millis(18), 40.0, 1e6).unwrap();
        assert!((dy - 32.0).abs() < 1e-3, "{dy}");
        assert_eq!(s.last_step(), Some(t0 + Duration::from_millis(18)));
    }

    #[test]
    fn scroller_bounces_at_the_document_ends() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut s = Scroller::new(4_000.0, t0, t0 + Duration::from_secs(10));
        // At the bottom it turns around…
        let dy = s.step(ms(10), 5_000.0, 5_000.0).unwrap();
        assert!(dy < 0.0, "{dy}");
        // …keeps going up…
        assert!(s.step(ms(20), 4_960.0, 5_000.0).unwrap() < 0.0);
        // …and turns again at the top.
        assert!(s.step(ms(30), 0.0, 5_000.0).unwrap() > 0.0);
        // Nothing to scroll: it stays put but keeps frames coming.
        assert_eq!(s.step(ms(40), 0.0, 0.0), Some(0.0));
    }

    #[test]
    fn scroller_stops_at_its_deadline() {
        let t0 = Instant::now();
        let until = t0 + Duration::from_secs(1);
        let mut s = Scroller::new(4_000.0, t0, until);
        assert!(s.step(t0 + Duration::from_millis(500), 0.0, 1e6).is_some());
        assert_eq!(s.step(until, 2_000.0, 1e6), None);
        // The last scroll event was the last step that moved.
        assert_eq!(s.last_step(), Some(t0 + Duration::from_millis(500)));
        assert_eq!(s.step(until + Duration::from_millis(8), 2_000.0, 1e6), None);
    }
}
