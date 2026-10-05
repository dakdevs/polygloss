//! `sections`: category sections at scale (plan T6.17, design §11.15,
//! §12.1 "Open or close a category section").
//!
//! Before the first frame the viewport gets a 72 pt prelude (the header
//! card's slot) and two sections ([`plan`]): the last 20 files in an open
//! one, and among the rest every file with an odd index in a closed one,
//! half the corpus behind one band. Then:
//!
//! - `sections_scroll_p95_ms`: the `scroll` scenario's definition (10 s at
//!   4,000 px/s plus 20 seeded jumps, CPU time per frame) over this
//!   document. Jumps go to shown files only: one into the closed section
//!   would open it.
//! - `section_toggle_ms`: with the closed section's band at the top, opening
//!   it → the next painted frame, p95 over 20 opens (it is closed again,
//!   untimed, between them).

use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{AsyncApp, IntoElement as _, Styled as _, div, px};
use polygloss_viewport::{RowKey, ScrollAnchor, Section};
use serde_json::json;

use super::{Rng, scroll};
use crate::args::Knobs;
use crate::harness::Harness;
use crate::metrics::{ScenarioResult, Series, ms};

/// The closed section of every other file, and the open one at the end.
pub const BIG: u32 = 1;
pub const LAST: u32 = 2;

/// Files in the open section at the end.
const LAST_FILES: u32 = 20;

/// The prelude's height in points.
const PRELUDE_H: f32 = 72.0;

/// Longest wait for the frame after opening the section.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// No frame for this long between toggles counts as idle.
const QUIET: Duration = Duration::from_millis(150);

/// The sections over a corpus of `files` files: the odd files before the
/// last 20 in a closed section ([`BIG`]), the last 20 in an open one
/// ([`LAST`]).
pub fn plan(files: u32) -> Vec<Section> {
    let last = files.saturating_sub(LAST_FILES);
    let odd: Vec<u32> = (0..last).filter(|f| f % 2 == 1).collect();
    let tail: Vec<u32> = (last..files).collect();
    vec![
        Section {
            id: BIG,
            label: format!("{} odd files", odd.len()).into(),
            icon: None,
            files: odd,
            open: false,
        },
        Section {
            id: LAST,
            label: format!("{} last files", tail.len()).into(),
            icon: None,
            files: tail,
            open: true,
        },
    ]
}

/// Gives the viewport its prelude and sections (before its first frame).
pub fn prepare(h: &Harness, cx: &mut AsyncApp) {
    h.update(cx, |v, cx| {
        let files = v.document().len();
        v.set_prelude(
            Some(Rc::new(|_, _| {
                div().w_full().h(px(PRELUDE_H)).into_any_element()
            })),
            cx,
        );
        v.set_sections(plan(files), cx);
    });
}

pub async fn run(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    rng: &mut Rng,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let (shown, band) = h.read(cx, |v| {
        let shown: Vec<u32> = v
            .display_order()
            .iter()
            .copied()
            .filter(|&f| !v.is_hidden(f))
            .collect();
        let doc = v.document();
        let band = doc
            .section_by_id(BIG)
            .and_then(|s| doc.sections()[s].files.first().copied());
        (shown, band)
    });
    anyhow::ensure!(!shown.is_empty(), "the corpus shows no file");
    scroll::measure(h, cx, knobs, rng, result, "sections_", &shown).await?;

    let band = band.context("the corpus is too small for a closed section")?;
    // The closed band at the top (an anchor in a closed section lands on
    // its band and never opens it).
    h.update(cx, |v, cx| {
        v.scroll_to_anchor(
            ScrollAnchor {
                file_idx: band,
                row: RowKey::Lead,
                offset_px: 0.0,
            },
            cx,
        )
    });
    h.quiet(cx, QUIET, FRAME_TIMEOUT).await?;
    let mut opens = Vec::new();
    for _ in 0..knobs.ops {
        h.drain();
        let started = Instant::now();
        h.update(cx, |v, cx| v.set_section_open(BIG, true, cx));
        let frame = h
            .next_frame(cx, FRAME_TIMEOUT)
            .await?
            .context("no frame was painted after opening the section")?;
        opens.push(ms(frame.at - started));
        h.quiet(cx, QUIET, FRAME_TIMEOUT).await?;
        h.update(cx, |v, cx| v.set_section_open(BIG, false, cx));
        h.quiet(cx, QUIET, FRAME_TIMEOUT).await?;
    }
    let series = Series::new(&opens);
    result.metric("section_toggle_ms", series.p95());
    result.metric("section_toggle_max_ms", series.max());
    let hidden = h.read(cx, |v| v.section_counts(BIG).files);
    result.info(
        "sections",
        json!({ "shown": shown.len(), "closed": hidden, "prelude_pt": PRELUDE_H }),
    );
    result.samples("toggle_ms", opens);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_puts_the_last_20_files_open_and_the_odd_rest_closed() {
        let sections = plan(30);
        assert_eq!(sections.len(), 2);
        let (big, last) = (&sections[0], &sections[1]);
        assert_eq!((big.id, big.open), (BIG, false));
        assert_eq!(big.files, [1, 3, 5, 7, 9]);
        assert_eq!(big.label.as_ref(), "5 odd files");
        assert_eq!((last.id, last.open), (LAST, true));
        assert_eq!(last.files, (10..30).collect::<Vec<u32>>());
        assert_eq!(last.label.as_ref(), "20 last files");
        // A corpus of 20 files or fewer is all in the open section.
        let small = plan(3);
        assert_eq!(small[0].files, Vec::<u32>::new());
        assert_eq!(small[1].files, [0, 1, 2]);
        assert_eq!(small[1].label.as_ref(), "3 last files");
    }
}
