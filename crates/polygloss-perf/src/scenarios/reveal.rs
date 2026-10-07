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
//!
//! Then the section variant (T7.11, ADR-0030 M4): the `sections`
//! scenario's plan ([`sections::plan`]: every other file behind a closed
//! band, the last 20 in an open section), its closed band at the top,
//! opened and closed [`Knobs::ops`] times by a synthesized pointer
//! (`toggle_section_by`, what the band's Show / Hide does).
//!
//! - `section_anim_p95_ms`: the p95 of those toggles' animation frames'
//!   prepaint + paint CPU time, opens and closes together.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{AsyncApp, Context, Window};
use polygloss_viewport::motion::{Initiator, MotionPolicy, MotionPolicyOverride};
use polygloss_viewport::{DiffViewport, Document, RowKey, ScrollAnchor};
use serde_json::json;

use super::sections;
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
            let card =
                move |v: &mut DiffViewport, w: &mut Window, cx: &mut Context<DiffViewport>| {
                    v.toggle_collapsed_by(f, Initiator::Pointer, w, cx)
                };
            let Some(t) = toggle(h, cx, card).await? else {
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
    section_variant(h, cx, knobs, result).await
}

/// The section variant: the plan's closed band at the top, opened and
/// closed by pointer `knobs.ops` times (`section_anim_p95_ms`).
async fn section_variant(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let band = h.update(cx, |v, cx| {
        let files = v.document().len();
        v.set_sections(sections::plan(files), cx);
        let doc = v.document();
        let s = doc.section_by_id(sections::BIG)?;
        let first = doc.sections()[s].files[0];
        v.scroll_to_anchor(
            ScrollAnchor {
                file_idx: first,
                row: RowKey::Lead,
                offset_px: 0.0,
            },
            cx,
        );
        Some(s)
    });
    let Some(s) = band else {
        // A corpus of 21 files or fewer has no closed section.
        result.metric("section_anim_p95_ms", None);
        return Ok(());
    };
    h.quiet(cx, QUIET, QUIET_CAP).await?;
    let (mut anim, mut timeouts) = (Vec::new(), 0);
    for _ in 0..knobs.ops {
        for _open in [true, false] {
            let band =
                move |v: &mut DiffViewport, w: &mut Window, cx: &mut Context<DiffViewport>| {
                    v.toggle_section_by(s, Initiator::Pointer, w, cx)
                };
            match toggle(h, cx, band).await? {
                Some(t) => anim.extend(t.frames.iter().skip(1).map(|r| ms(r.cpu()))),
                None => timeouts += 1,
            }
        }
    }
    result.metric("section_anim_p95_ms", Series::new(&anim).p95());
    result.info(
        "section_reveal",
        json!({
            "toggles": 2 * knobs.ops,
            "anim_frames": anim.len(),
            "timeouts": timeouts,
            "anim_max_ms": Series::new(&anim).max(),
        }),
    );
    result.samples("section_anim_ms", anim);
    Ok(())
}

/// Runs `input` (a toggle by pointer) at the start of the next display
/// frame and collects the frames until quiet; `None` when it drew nothing.
async fn toggle(
    h: &mut Harness,
    cx: &mut AsyncApp,
    input: impl FnOnce(&mut DiffViewport, &mut Window, &mut Context<DiffViewport>) + 'static,
) -> anyhow::Result<Option<Toggle>> {
    h.drain();
    let ran = Rc::new(Cell::new(None::<Instant>));
    let stamp = ran.clone();
    h.at_next_frame(cx, move |v, window, cx| {
        stamp.set(Some(Instant::now()));
        input(v, window, cx);
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
