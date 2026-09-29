//! `open`: `first_paint_ms`, process start → the first frame in which every
//! visible row is painted (plain text allowed: no row still loading). It
//! includes everything a user waits for: process launch, resolve and
//! `diff-tree` in a fresh store, the window, and the first files' loads.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::AsyncApp;
use serde_json::json;

use crate::harness::{FrameRecord, Harness};
use crate::metrics::{Clock, ScenarioResult, ms};

/// Longest wait for first paint (the Linux budget is 2 s).
const TIMEOUT: Duration = Duration::from_secs(120);

/// First paint as seen in a run's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstPaint {
    /// Process start → the first frame without loading rows.
    pub first_paint: Duration,
    /// Process start → the first frame of all.
    pub first_frame: Duration,
    /// Frames up to and including first paint.
    pub frames: usize,
}

impl FirstPaint {
    /// Finds first paint among `frames` (in order). `overhead` is harness
    /// time inside the interval that is not the app's (the manifest lookup
    /// when `--repo` was not given) and is left out.
    pub fn from_frames(
        start: Instant,
        overhead: Duration,
        frames: &[FrameRecord],
    ) -> Option<FirstPaint> {
        let first = frames.first()?;
        let (i, painted) = frames
            .iter()
            .enumerate()
            .find(|(_, f)| f.stats.loading_rows == 0)?;
        let since = |f: &FrameRecord| {
            f.at.saturating_duration_since(start)
                .saturating_sub(overhead)
        };
        Some(FirstPaint {
            first_paint: since(painted),
            first_frame: since(first),
            frames: i + 1,
        })
    }
}

pub async fn run(
    h: &mut Harness,
    cx: &AsyncApp,
    clock: Clock,
    overhead: Duration,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    h.wait_for(cx, TIMEOUT, |f| f.stats.loading_rows == 0)
        .await?
        .context("the visible rows never finished loading")?;
    let fp = FirstPaint::from_frames(clock.process_start, overhead, &h.seen)
        .context("no first paint among the frames")?;
    result.metric("first_paint_ms", Some(ms(fp.first_paint)));
    result.info(
        "first_paint",
        json!({
            "first_frame_ms": ms(fp.first_frame),
            "frames": fp.frames,
            "window_to_first_paint_ms": ms(h.seen[fp.frames - 1].at.saturating_duration_since(h.opened_at)),
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use polygloss_viewport::FrameStats;

    use super::*;
    use crate::harness::FrameRecord;

    fn frame(at: Instant, loading_rows: u32) -> FrameRecord {
        FrameRecord {
            at,
            stats: FrameStats {
                prepaint: Duration::from_millis(1),
                paint: Duration::from_millis(1),
                visible_rows: 40,
                shaped_lines: 40,
                loading_rows,
                unhighlighted_rows: loading_rows,
            },
        }
    }

    #[test]
    fn first_paint_is_the_first_frame_without_loading_rows() {
        let start = Instant::now();
        let ms = |n| start + Duration::from_millis(n);
        let frames = [
            frame(ms(90), 12),
            frame(ms(110), 3),
            frame(ms(140), 0),
            frame(ms(150), 0),
        ];
        let fp = FirstPaint::from_frames(start, Duration::ZERO, &frames).unwrap();
        assert_eq!(fp.first_paint, Duration::from_millis(140));
        assert_eq!(fp.first_frame, Duration::from_millis(90));
        assert_eq!(fp.frames, 3);
        // Harness overhead (the manifest lookup) is not app time.
        let fp = FirstPaint::from_frames(start, Duration::from_millis(40), &frames).unwrap();
        assert_eq!(fp.first_paint, Duration::from_millis(100));
        assert!(FirstPaint::from_frames(start, Duration::ZERO, &frames[..2]).is_none());
    }
}
