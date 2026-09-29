//! `blocks`: `comment_repaint_ms` in M2, `set_blocks` adding one 6-line
//! block on a visible file → the next completed frame, p95 over 20 ops.
//!
//! Each op anchors a new block (a comment-like card, six lines of text) at
//! the code line nearest the middle of the viewport that has none yet (so it
//! lands in view), keeps the file's earlier blocks, and times from the call
//! to the frame that shows it (the viewport measures a new block in that same
//! frame, T2.7). From M3 the app's `comment-roundtrip` scenario replaces it
//! (T3.10).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{AsyncApp, IntoElement as _, ParentElement as _, Styled as _, div, rgb};
use polygloss_diff::Side;
use polygloss_viewport::{BlockAnchor, BlockId, BlockSpec, BodyRow, Document, RenderBlock};
use serde_json::json;

use crate::args::Knobs;
use crate::harness::Harness;
use crate::metrics::{ScenarioResult, Series, ms};

/// The block's text: six lines, like a short review comment.
pub const COMMENT_LINES: [&str; 6] = [
    "perf-bot · just now",
    "This allocates a new String for every row on the hot path.",
    "Could the label be cached per (file, row) like the shaped lines?",
    "The shaped-line cache already keys on the row, so the lookup is free.",
    "Not blocking, but it shows up in the scroll profile on large diffs.",
    "Happy to take a follow-up if you'd rather land this first.",
];

/// Longest wait for the frame after `set_blocks`.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// No frame for this long between ops counts as idle.
const QUIET: Duration = Duration::from_millis(150);

/// Where to put a block so it shows: the code line nearest the middle of
/// the viewport (at or below it first, then above) that has no block yet
/// (`used`), in whichever visible, laid-out file has one; else the top of
/// the file in the middle.
pub fn visible_line(
    doc: &Document,
    used: &HashSet<(u32, BlockAnchor)>,
) -> Option<(u32, BlockAnchor)> {
    if doc.is_empty() {
        return None;
    }
    let height = f64::from(doc.viewport_height());
    let top = doc.scroll_top();
    let (bottom, mid) = (top + height, top + height / 2.0);
    let header = f64::from(doc.metrics().header_height);
    let (mid_file, _) = doc.file_at(mid);
    let free = |f: u32| move |a: &BlockAnchor| !used.contains(&(f, *a));
    let last = doc.file_at(bottom).0;
    for f in mid_file..=last {
        if let Some(a) = lines_in(doc, f, mid, bottom).into_iter().find(free(f)) {
            return Some((f, a));
        }
    }
    let first = doc.file_at(top).0;
    for f in (first..=mid_file).rev() {
        // Rows right under the (pinned) header at the top are covered.
        let lines = lines_in(doc, f, top + header, mid);
        if let Some(a) = lines.into_iter().rev().find(free(f)) {
            return Some((f, a));
        }
    }
    Some((mid_file, BlockAnchor::FileTop))
}

/// The code lines of file `f` whose rows start within `[from, to)` in
/// document coordinates, top to bottom.
fn lines_in(doc: &Document, f: u32, from: f64, to: f64) -> Vec<BlockAnchor> {
    let Some(layout) = doc.file_layout(f).filter(|_| !doc.is_collapsed(f)) else {
        return Vec::new();
    };
    let body_top = doc.file_top(f) + f64::from(doc.metrics().header_height);
    let lo = (from - body_top).max(0.0);
    let hi = (to - body_top).min(layout.height());
    if lo >= hi || layout.is_empty() {
        return Vec::new();
    }
    let (first, _) = layout.row_at(lo);
    (first..layout.len())
        .take_while(|&i| layout.row_top(i) < hi)
        .filter_map(|i| match layout.rows()[i] {
            BodyRow::Line {
                new: Some(line), ..
            } => Some(BlockAnchor::Line {
                side: Side::New,
                line,
            }),
            BodyRow::Line {
                old: Some(line), ..
            } => Some(BlockAnchor::Line {
                side: Side::Old,
                line,
            }),
            _ => None,
        })
        .collect()
}

/// A comment-like card: a bordered box with [`COMMENT_LINES`].
fn comment_block() -> RenderBlock {
    Rc::new(|_, _| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .mx_2()
            .my_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xd0d7de))
            .bg(rgb(0xf6f8fa))
            .text_sm()
            .text_color(rgb(0x1f2328))
            .children(COMMENT_LINES.iter().map(|line| div().child(*line)))
            .into_any_element()
    })
}

pub async fn run(
    h: &mut Harness,
    cx: &mut AsyncApp,
    knobs: &Knobs,
    result: &mut ScenarioResult,
) -> anyhow::Result<()> {
    let mut per_file: HashMap<u32, Vec<BlockSpec>> = HashMap::new();
    let mut used = HashSet::new();
    let mut ops = Vec::new();
    let mut placed = Vec::new();
    for i in 0..knobs.ops {
        let (f, anchor) = h
            .read(cx, |v| visible_line(v.document(), &used))
            .context("no file in view to add a block to")?;
        used.insert((f, anchor));
        let blocks = per_file.entry(f).or_default();
        blocks.push(BlockSpec {
            id: BlockId(u64::from(i) + 1),
            anchor,
            render: comment_block(),
        });
        let blocks = blocks.clone();
        placed.push(json!({ "file": f, "anchor": format!("{anchor:?}") }));
        h.drain();
        let started = Instant::now();
        h.update(cx, |v, cx| v.set_blocks(f, blocks, cx));
        let frame = h
            .next_frame(cx, FRAME_TIMEOUT)
            .await?
            .context("no frame was painted after set_blocks")?;
        ops.push(ms(frame.at - started));
        h.quiet(cx, QUIET, FRAME_TIMEOUT).await?;
    }
    let series = Series::new(&ops);
    result.metric("comment_repaint_ms", series.p95());
    result.metric("comment_repaint_max_ms", series.max());
    result.info("blocks", json!({ "placed": placed }));
    result.samples("op_ms", ops);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use polygloss_diff::{
        FileChange, FileKind, FileStatus, GitPath, Mode, ObjectFormat, Oid, Side,
    };
    use polygloss_viewport::{BlockAnchor, BodyRow, Document, FileLayout, Metrics};

    use super::*;

    fn change(idx: u32) -> FileChange {
        let p = Some(GitPath::from_bytes(format!("src/f{idx}.rs").as_bytes()));
        FileChange {
            idx,
            status: FileStatus::Modified,
            old_path: p.clone(),
            new_path: p,
            old_mode: Some(Mode(0o100644)),
            new_mode: Some(Mode(0o100644)),
            old_blob: Oid::parse(&format!("{:040x}", 2 * idx + 1), ObjectFormat::Sha1).unwrap(),
            new_blob: Oid::parse(&format!("{:040x}", 2 * idx + 2), ObjectFormat::Sha1).unwrap(),
            similarity: None,
            kind: FileKind::Text,
            generated: false,
        }
    }

    /// `n` files of `rows` rows each: a removed line, then context lines.
    fn doc(n: u32, rows: u32) -> Document {
        let mut doc = Document::new(Arc::new((0..n).map(change).collect()), Metrics::default());
        doc.set_viewport_height(900.0);
        for f in 0..n {
            let body: Vec<BodyRow> = (0..rows)
                .map(|i| match i {
                    0 => BodyRow::Line {
                        old: Some(0),
                        new: None,
                        diff_row: 0,
                    },
                    _ => BodyRow::Line {
                        old: Some(i),
                        new: Some(i - 1),
                        diff_row: i,
                    },
                })
                .collect();
            let heights = vec![20.0; body.len()];
            doc.set_file_layout(f, FileLayout::new(body, &heights));
        }
        doc
    }

    #[test]
    fn picks_a_line_near_the_middle_of_the_viewport() {
        let mut doc = doc(3, 100);
        // File 0 is 40 + 2,000 px tall: the middle (y = 1,000 + 450) is in it.
        doc.scroll_by(1_000.0);
        let (f, anchor) = visible_line(&doc, &HashSet::new()).unwrap();
        assert_eq!(f, 0);
        let BlockAnchor::Line { side, line } = anchor else {
            panic!("{anchor:?}")
        };
        assert_eq!(side, Side::New);
        // Row at 1,450 - 40 = 1,410 px into the body: row 70, new line 69.
        assert_eq!(line, 69);
    }

    #[test]
    fn a_removed_line_anchors_on_the_old_side() {
        let doc = doc(20, 1);
        // Every body is its removed line; the middle of the first screen is
        // in file 7 (60 px per file).
        let (f, anchor) = visible_line(&doc, &HashSet::new()).unwrap();
        assert_eq!(f, 7);
        assert_eq!(
            anchor,
            BlockAnchor::Line {
                side: Side::Old,
                line: 0
            }
        );
    }

    #[test]
    fn falls_back_to_a_visible_line_or_the_file_top() {
        let mut doc = doc(3, 100);
        // The middle of the viewport is on file 1's header: the nearest
        // line row below it is file 1's first.
        doc.scroll_by((2_040.0 - 450.0 + 10.0) as f32);
        let (f, anchor) = visible_line(&doc, &HashSet::new()).unwrap();
        assert_eq!(f, 1);
        assert_eq!(
            anchor,
            BlockAnchor::Line {
                side: Side::Old,
                line: 0
            }
        );

        // Placeholder bodies have no lines: the block goes at the top of the
        // file in the middle of the viewport.
        let mut placeholders =
            Document::new(Arc::new((0..30).map(change).collect()), Metrics::default());
        placeholders.set_viewport_height(900.0);
        for f in 0..30 {
            placeholders.set_file_layout(f, FileLayout::placeholder(48.0));
        }
        let (f, anchor) = visible_line(&placeholders, &HashSet::new()).unwrap();
        // 88 px per file: y = 450 is in file 5.
        assert_eq!((f, anchor), (5, BlockAnchor::FileTop));

        let empty = Document::new(Arc::new(Vec::new()), Metrics::default());
        assert_eq!(visible_line(&empty, &HashSet::new()), None);
    }

    #[test]
    fn lines_that_already_have_a_block_are_skipped() {
        let mut doc = doc(3, 100);
        doc.scroll_by(1_000.0);
        let line = |n| BlockAnchor::Line {
            side: Side::New,
            line: n,
        };
        // New line 69 (the middle) has a block: the next line below it.
        let used = HashSet::from([(0, line(69))]);
        assert_eq!(visible_line(&doc, &used), Some((0, line(70))));
        // Every line from the middle to the bottom has one: the nearest
        // line above the middle.
        let used: HashSet<_> = (69..=92).map(|n| (0, line(n))).collect();
        assert_eq!(visible_line(&doc, &used), Some((0, line(68))));
    }

    #[test]
    fn the_comment_block_has_six_lines() {
        assert_eq!(COMMENT_LINES.len(), 6);
        assert!(COMMENT_LINES.iter().all(|l| !l.is_empty()));
    }
}
