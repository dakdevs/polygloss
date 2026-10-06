//! `reveal`: the card reveal (T7.8, ADR-0030 M3; design §12.1 "Animation
//! frame, viewport only" and "Commit frame of an animated toggle").
//!
//! Under the Full motion policy, each of the first [`Knobs::ops`] shown
//! files (20) in turn is brought to the top of the viewport, its rows
//! loaded and highlighted, and then collapsed and expanded by a
//! synthesized pointer (`toggle_collapsed_by` with `Initiator::Pointer`,
//! what the header chevron's click does). Each toggle runs at the start of
//! a display frame; its first frame is the commit frame, and every frame
//! after it until none comes for [`QUIET`] is an animation frame.
//!
//! - `collapse_anim_p95_ms`: the p95 of the animation frames' prepaint +
//!   paint CPU time (as `scroll_p95_ms`), collapses and expands together.
//! - `collapse_commit_ms`: the expands' input → the end of their commit
//!   frame's paint, p95 (the commit lays a body out and shapes its rows).

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::AsyncApp;
use polygloss_viewport::Document;
use polygloss_viewport::motion::{Initiator, MotionPolicy, MotionPolicyOverride};
use serde_json::json;

use crate::args::Knobs;
use crate::harness::{FrameRecord, Harness};
use crate::metrics::{ScenarioResult, Series, ms};

/// No frame for this long ends a toggle (a reveal lasts 200 ms at most and
/// draws every display frame).
const QUIET: Duration = Duration::from_millis(250);
/// Longest wait for a card's rows to load and highlight at the top.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait for quiet before a toggle.
const QUIET_CAP: Duration = Duration::from_secs(10);
/// Longest wait for the display frame that runs a toggle.
const INPUT_TIMEOUT: Duration = Duration::from_secs(5);

/// The first `n` shown files, in display order: the cards the run toggles.
pub fn cards(doc: &Document, n: usize) -> Vec<u32> {
    doc.display_order()
        .iter()
        .copied()
        .filter(|&f| !doc.is_hidden(f))
        .take(n)
        .collect()
}

/// One toggle: when its input ran and the frames drawn after it.
struct Toggle {
    input: Instant,
    frames: Vec<FrameRecord>,
}

pub async fn run(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    cx.update(|cx| cx.set_global(MotionPolicyOverride(Some(MotionPolicy::Full))));
    let cards = h.read(cx, |v| cards(v.document(), knobs.ops as usize));
    anyhow::ensure!(!cards.is_empty(), "the corpus shows no file");
    let (mut anim, mut commits, mut timeouts) = (Vec::new(), Vec::new(), 0);
    for &f in &cards {
        h.jump_to_file(cx, f);
        h.wait_for(cx, READY_TIMEOUT, |r| {
            r.stats.loading_rows == 0 && r.stats.unhighlighted_rows == 0
        })
        .await?
        .with_context(|| format!("file {f} never finished loading at the top"))?;
        h.quiet(cx, QUIET, QUIET_CAP).await?;
        for expand in [false, true] {
            let Some(t) = toggle(h, cx, f).await? else {
                timeouts += 1;
                continue;
            };
            anim.extend(t.frames.iter().skip(1).map(|r| ms(r.cpu())));
            if expand {
                commits.push(ms(t.frames[0].at.saturating_duration_since(t.input)));
            }
        }
    }
    result.metric("collapse_anim_p95_ms", Series::new(&anim).p95());
    result.metric("collapse_commit_ms", Series::new(&commits).p95());
    result.info(
        "reveal",
        json!({
            "cards": cards.len(),
            "anim_frames": anim.len(),
            "timeouts": timeouts,
            "anim_max_ms": Series::new(&anim).max(),
            "commit_max_ms": Series::new(&commits).max(),
        }),
    );
    result.samples("anim_ms", anim);
    result.samples("commit_ms", commits);
    Ok(())
}

/// Toggles file `f` by pointer at the start of the next display frame and
/// collects the frames until quiet; `None` when it drew nothing.
async fn toggle(h: &mut Harness, cx: &mut AsyncApp, f: u32) -> anyhow::Result<Option<Toggle>> {
    h.drain();
    let ran = Rc::new(Cell::new(None::<Instant>));
    let stamp = ran.clone();
    h.at_next_frame(cx, move |v, window, cx| {
        stamp.set(Some(Instant::now()));
        v.toggle_collapsed_by(f, Initiator::Pointer, window, cx);
    })?;
    let asked = Instant::now();
    let input = loop {
        if let Some(input) = ran.get() {
            break input;
        }
        anyhow::ensure!(
            asked.elapsed() < INPUT_TIMEOUT,
            "no display frame ran the toggle"
        );
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    };
    let mut frames: Vec<FrameRecord> = h.drain().into_iter().filter(|r| r.at >= input).collect();
    while let Some(frame) = h.next_frame(cx, QUIET).await? {
        frames.push(frame);
    }
    Ok((!frames.is_empty()).then_some(Toggle { input, frames }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use polygloss_diff::{
        FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, ObjectFormat, Oid,
    };
    use polygloss_viewport::Metrics;

    use super::*;

    fn change(idx: u32) -> FileChange {
        FileChange {
            idx,
            status: FileStatus::Added,
            old_path: None,
            new_path: Some(GitPath::from_bytes(format!("f{idx}.rs").as_bytes())),
            old_mode: None,
            new_mode: None,
            old_blob: Oid::zero(ObjectFormat::Sha1),
            new_blob: Oid::parse(&format!("{:040x}", idx + 1), ObjectFormat::Sha1).unwrap(),
            similarity: None,
            kind: FileKind::Text,
            generated: false,
            generated_attr: GeneratedAttr::Unspecified,
        }
    }

    #[test]
    fn cards_are_the_first_shown_files_in_display_order() {
        let files = Arc::new((0..6).map(change).collect::<Vec<_>>());
        let mut doc = Document::new(files, Metrics::default());
        doc.set_order(vec![5, 4, 3, 2, 1, 0]);
        doc.set_hidden(&[4], true);
        assert_eq!(cards(&doc, 3), [5, 3, 2]);
        assert_eq!(cards(&doc, 20), [5, 3, 2, 1, 0]);
    }
}
