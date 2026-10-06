//! The scenarios (plan T2.9 metric definitions). Each runs in its own
//! process over one corpus and layout and fills in a [`ScenarioResult`]:
//!
//! | Scenario    | Metric               | What is timed                                                |
//! | ----------- | -------------------- | ------------------------------------------------------------ |
//! | `open`      | `first_paint_ms`     | process start → first frame with no loading rows             |
//! | `scroll`    | `scroll_p95_ms`      | prepaint + paint per frame over 10 s at 4,000 px/s + 20 jumps |
//! | `highlight` | `highlight_ms`       | last scroll event → first frame with every visible row highlighted, p95 of 20 stops |
//! | `blocks`    | `comment_repaint_ms` | `set_blocks` adding a 6-line block on a visible file → next frame, p95 of 20 |
//! | `sections`  | `sections_scroll_p95_ms`, `section_toggle_ms` | `scroll` over a document with category sections; opening a big closed section → next frame, p95 of 20 |
//! | `reveal`    | `collapse_anim_p95_ms`, `collapse_commit_ms` | 20 cards collapsed and expanded by pointer (T7.8): prepaint + paint of the animation frames, p95; an expand's input → its commit frame, p95 |
//!
//! All but `open` first [`settle`]: wait for first paint, then for the
//! visible rows' tokens, then for a quiet window. Random choices come from a
//! seeded [`Rng`], so runs are repeatable.

pub mod blocks;
pub mod highlight;
pub mod open;
pub mod reveal;
pub mod scroll;
pub mod sections;

use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::AsyncApp;
use serde_json::json;

use crate::args::{Args, ScenarioName};
use crate::harness::Harness;
use crate::metrics::{Clock, ScenarioResult, ms};

/// Longest wait for first paint (the Linux budget is 2 s).
const FIRST_PAINT_TIMEOUT: Duration = Duration::from_secs(120);
/// Longest wait for the first screen's tokens while settling.
const HIGHLIGHT_TIMEOUT: Duration = Duration::from_secs(60);
/// No frame for this long counts as idle.
const QUIET: Duration = Duration::from_millis(200);
/// Longest wait for idle.
const QUIET_CAP: Duration = Duration::from_secs(10);

/// Runs `args.scenario` in the harness's window.
pub async fn run(
    h: &mut Harness,
    cx: &mut AsyncApp,
    args: &Args,
    clock: Clock,
    overhead: Duration,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let mut rng = Rng::new(args.knobs.seed);
    match args.scenario {
        ScenarioName::Open => open::run(h, cx, clock, overhead, result).await,
        ScenarioName::Scroll => {
            settle(h, cx, clock, result).await?;
            scroll::run(h, cx, &args.knobs, &mut rng, result).await
        }
        ScenarioName::Highlight => {
            settle(h, cx, clock, result).await?;
            highlight::run(h, cx, &args.knobs, &mut rng, result).await
        }
        ScenarioName::Blocks => {
            settle(h, cx, clock, result).await?;
            blocks::run(h, cx, &args.knobs, result).await
        }
        ScenarioName::Sections => {
            // Before the first frame, as a review tab partitions its files.
            sections::prepare(h, cx);
            settle(h, cx, clock, result).await?;
            sections::run(h, cx, &args.knobs, &mut rng, result).await
        }
        ScenarioName::Reveal => {
            settle(h, cx, clock, result).await?;
            reveal::run(h, cx, &args.knobs, result).await
        }
    }
}

/// Waits for first paint, then for the visible rows' tokens, then until no
/// frame has come for a while, so a scenario starts from a settled screen.
pub async fn settle(
    h: &mut Harness,
    cx: &AsyncApp,
    clock: Clock,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let painted = h
        .wait_for(cx, FIRST_PAINT_TIMEOUT, |f| f.stats.loading_rows == 0)
        .await?
        .context("settling: the visible rows never finished loading")?;
    let highlighted = h
        .wait_for(cx, HIGHLIGHT_TIMEOUT, |f| f.stats.unhighlighted_rows == 0)
        .await?
        .context("settling: the visible rows never got their tokens")?;
    let quiet = h.quiet(cx, QUIET, QUIET_CAP).await?;
    result.info(
        "settle",
        json!({
            "first_paint_ms": ms(painted.at - clock.process_start),
            "highlighted_ms": ms(highlighted.at - clock.process_start),
            "quiet": quiet,
        }),
    );
    Ok(())
}

/// A small seeded generator (SplitMix64): every random choice of a run
/// comes from one, so the same seed replays the same run.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_per_seed() {
        let draw = |seed| {
            let mut rng = Rng::new(seed);
            (0..8).map(|_| rng.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(draw(1), draw(1));
        assert_ne!(draw(1), draw(2));
        // Seed 0 is a valid seed, not a stuck generator.
        let zero = draw(0);
        assert!(zero.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn rng_draws_stay_in_range() {
        let mut rng = Rng::new(7);
        for _ in 0..10_000 {
            assert!(rng.below(13) < 13);
            let u = rng.unit();
            assert!((0.0..1.0).contains(&u), "{u}");
        }
        assert_eq!(rng.below(1), 0);
        assert_eq!(rng.below(0), 0);
    }
}
