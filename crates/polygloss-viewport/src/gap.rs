//! Hidden context (design §11.6 "Context"): gap rows with their expanders
//! ("↑ 20", "↓ 20", "Expand all"), and each file's revealed old-side ranges
//! ([`Expansions`], which view state persists, design §11.12). Hosts also
//! reveal one hidden line with [`DiffViewport::reveal_line`] (going to a
//! thread or a find match inside collapsed context).
//!
//! A file with nothing revealed paints the rows its [`MaterializedFile`]
//! shares between versions. Revealing context builds that file's rows again
//! with its expansions (linear in its rows, on the main thread, once per
//! change) and lays it out on the spot; the scroll anchor keeps everything
//! above the change still.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::Context;
use polygloss_diff::Side;
use polygloss_diff::hunks::FileDiff;
use polygloss_diff::rows::{ExpandBy, Expansions, GapId, Layout, Row, build_rows};

use crate::controls::ControlAction;
use crate::document::{BodyRow, FileState};
use crate::materialize::MaterializedFile;
use crate::paint_rows::{FULL, Painter};
use crate::view::DiffViewport;

/// Lines one "↑"/"↓" click reveals (design §11.6: 20).
pub const EXPAND_STEP: u32 = 20;

/// Lines [`DiffViewport::reveal_line`] reveals above and below a hidden line.
pub const REVEAL_CONTEXT: u32 = 3;

/// Revealed context per file, and the rows built with it.
#[derive(Default)]
pub(crate) struct Gaps {
    files: HashMap<u32, FileGaps>,
    /// Lines to reveal once their file's diff is loaded
    /// ([`DiffViewport::reveal_line`] before it was).
    deferred: HashMap<u32, Vec<(Side, u32)>>,
}

#[derive(Default)]
struct FileGaps {
    expansions: Expansions,
    /// Bumped by every change (never reused), so a layout built for older
    /// expansions is stale.
    version: u64,
    rows: Option<ExpandedRows>,
}

/// Rows of one diff in one layout, built with expansions `version`.
struct ExpandedRows {
    layout: Layout,
    version: u64,
    diff: Arc<FileDiff>,
    rows: Vec<Row>,
}

impl ExpandedRows {
    fn matches(&self, layout: Layout, version: u64, file: &MaterializedFile) -> bool {
        self.layout == layout && self.version == version && Arc::ptr_eq(&self.diff, &file.diff)
    }
}

impl Gaps {
    /// The expansions version of file `f` (part of its layout key).
    pub(crate) fn version(&self, f: u32) -> u64 {
        self.files.get(&f).map_or(0, |g| g.version)
    }

    /// Applies `change` to file `f`'s expansions; returns whether they
    /// changed.
    fn update(&mut self, f: u32, change: impl FnOnce(&mut Expansions)) -> bool {
        let g = self.files.entry(f).or_default();
        let before = g.expansions.clone();
        change(&mut g.expansions);
        if g.expansions == before {
            return false;
        }
        g.version += 1;
        g.rows = None;
        true
    }

    /// The rows file `f` shows in `layout`: built with its expansions (and
    /// kept for painting), or the file's shared rows when nothing is
    /// revealed.
    pub(crate) fn rows<'a>(
        &'a mut self,
        f: u32,
        file: &'a MaterializedFile,
        layout: Layout,
    ) -> &'a [Row] {
        match self.files.get_mut(&f) {
            Some(g) if !g.expansions.is_empty() => {
                let fresh = g
                    .rows
                    .as_ref()
                    .is_some_and(|r| r.matches(layout, g.version, file));
                if !fresh {
                    g.rows = Some(ExpandedRows {
                        layout,
                        version: g.version,
                        diff: file.diff.clone(),
                        rows: build_rows(&file.diff, &g.expansions, layout),
                    });
                }
                &g.rows.as_ref().expect("just built").rows
            }
            _ => file.rows(layout),
        }
    }

    /// The rows the current layout of file `f` was built from (see
    /// [`Gaps::rows`]).
    pub(crate) fn painted_rows<'a>(
        &'a self,
        f: u32,
        file: &'a MaterializedFile,
        layout: Layout,
    ) -> &'a [Row] {
        match self.files.get(&f) {
            Some(g) if !g.expansions.is_empty() => match &g.rows {
                Some(r) if r.matches(layout, g.version, file) => &r.rows,
                _ => &[],
            },
            _ => file.rows(layout),
        }
    }

    /// Applies the reveals deferred for file `f` now that its diff `fd` is
    /// loaded; returns whether its expansions changed.
    fn apply_deferred(&mut self, f: u32, fd: &FileDiff) -> bool {
        let Some(lines) = self.deferred.remove(&f) else {
            return false;
        };
        let mut changed = false;
        for (side, line) in lines {
            if let Some(range) = hidden_reveal(fd, side, line) {
                changed |= self.update(f, |e| e.reveal(range));
            }
        }
        changed
    }

    /// Drops file `f`'s built rows (its data was evicted); its expansions
    /// stay.
    pub(crate) fn forget_rows(&mut self, f: u32) {
        if let Some(g) = self.files.get_mut(&f) {
            g.rows = None;
        }
    }
}

/// The old lines to reveal around line `line` of `side` when no hunk of
/// `fd` shows it (context between hunks, before the first or after the
/// last); `None` when a hunk shows it or it is past the end.
fn hidden_reveal(fd: &FileDiff, side: Side, line: u32) -> Option<Range<u32>> {
    let range = |h: &polygloss_diff::hunks::Hunk| match side {
        Side::Old => h.old.clone(),
        Side::New => h.new.clone(),
    };
    // Unchanged lines keep the offset the last hunk above them leaves.
    let mut old = line;
    for h in &fd.hunks {
        let r = range(h);
        if r.contains(&line) {
            return None;
        }
        if r.end > line {
            break;
        }
        old = h.old.end + (line - r.end);
    }
    let len = match side {
        Side::Old => fd.old.len(),
        Side::New => fd.new.len(),
    };
    if line >= len || old >= fd.old.len() {
        return None;
    }
    Some(old.saturating_sub(REVEAL_CONTEXT)..(old + REVEAL_CONTEXT + 1).min(fd.old.len()))
}

/// The old lines an expander on hidden run `run` reveals.
fn run_reveal(run: Range<u32>, by: ExpandBy) -> Range<u32> {
    match by {
        ExpandBy::All => run,
        ExpandBy::Up(n) => run.end.saturating_sub(n).max(run.start)..run.end,
        ExpandBy::Down(n) => run.start..run.start.saturating_add(n).min(run.end),
    }
}

impl DiffViewport {
    /// Reveals hidden context of gap `gap` in file `file_idx` (design §11.6
    /// "↑20 / ↓20 / Expand all"): `Up(n)` the `n` lines just above the hunk
    /// below the gap, `Down(n)` the `n` just below the hunk above it. Nothing
    /// above the gap moves. Ignored while the file's diff is not loaded (gap
    /// ids come from it).
    pub fn expand(&mut self, file_idx: u32, gap: GapId, by: ExpandBy, cx: &mut Context<Self>) {
        let Some(file) = self.loaded(file_idx) else {
            return;
        };
        if self
            .gaps
            .update(file_idx, |e| e.expand(&file.diff, gap, by))
        {
            self.after_reveal(file_idx, cx);
        }
    }

    /// Reveals a clicked expander's lines: `by` applied to the hidden run
    /// `run` it was painted on.
    pub(crate) fn expand_run(
        &mut self,
        file_idx: u32,
        run: Range<u32>,
        by: ExpandBy,
        cx: &mut Context<Self>,
    ) {
        if file_idx < self.doc.len()
            && self
                .gaps
                .update(file_idx, |e| e.reveal(run_reveal(run, by)))
        {
            self.after_reveal(file_idx, cx);
        }
    }

    /// Reveals every line of file `file_idx` (`E`, "Expand all" in the file
    /// menu). Before its diff is loaded this reveals `0..u32::MAX`, which
    /// shows the whole file once it is.
    pub fn expand_file(&mut self, file_idx: u32, cx: &mut Context<Self>) {
        if file_idx >= self.doc.len() {
            return;
        }
        let file = self.loaded(file_idx);
        let changed = self.gaps.update(file_idx, |e| match &file {
            Some(file) => e.expand_file(&file.diff),
            None => e.reveal(0..u32::MAX),
        });
        if changed {
            self.after_reveal(file_idx, cx);
        }
    }

    /// Replaces file `file_idx`'s revealed context with `ranges` (0-based,
    /// half-open old lines, as [`DiffViewport::expansions`] returns them), to
    /// restore view state (design §11.12).
    pub fn set_expansions(&mut self, file_idx: u32, ranges: &[[u32; 2]], cx: &mut Context<Self>) {
        if file_idx < self.doc.len()
            && self
                .gaps
                .update(file_idx, |e| *e = Expansions::from_ranges(ranges))
        {
            self.after_reveal(file_idx, cx);
        }
    }

    /// Every file with revealed context and its old-line ranges, in file
    /// order (what view state persists).
    pub fn expansions(&self) -> Vec<(u32, Vec<[u32; 2]>)> {
        let mut out: Vec<(u32, Vec<[u32; 2]>)> = self
            .gaps
            .files
            .iter()
            .filter(|(_, g)| !g.expansions.is_empty())
            .map(|(&f, g)| (f, g.expansions.to_ranges()))
            .collect();
        out.sort_unstable_by_key(|(f, _)| *f);
        out
    }

    /// Reveals line `line` (0-based) of `side` of file `file_idx` when it is
    /// hidden context (in a gap), with [`REVEAL_CONTEXT`] lines above and
    /// below, so that a host can put the cursor on it (going to a thread or
    /// a find match). Lines already shown change nothing. Before the file's
    /// diff is loaded the reveal waits for it and applies when the file is
    /// laid out.
    pub fn reveal_line(&mut self, file_idx: u32, side: Side, line: u32, cx: &mut Context<Self>) {
        if file_idx >= self.doc.len() {
            return;
        }
        match self.loaded(file_idx) {
            Some(file) => {
                if let Some(range) = hidden_reveal(&file.diff, side, line)
                    && self.gaps.update(file_idx, |e| e.reveal(range))
                {
                    self.after_reveal(file_idx, cx);
                }
            }
            None => self
                .gaps
                .deferred
                .entry(file_idx)
                .or_default()
                .push((side, line)),
        }
    }

    /// Applies file `f`'s deferred reveals when its diff is loaded (before
    /// its layout key is taken, so the layout is built once).
    pub(crate) fn apply_deferred_reveals(&mut self, f: u32) {
        if !self.gaps.deferred.contains_key(&f) {
            return;
        }
        if let Some(file) = self.loaded(f) {
            self.gaps.apply_deferred(f, &file.diff);
        }
    }

    fn loaded(&self, f: u32) -> Option<Arc<MaterializedFile>> {
        if f >= self.doc.len() {
            return None;
        }
        match self.doc.state(f) {
            FileState::Materialized(file) => Some(file.clone()),
            _ => None,
        }
    }

    fn after_reveal(&mut self, f: u32, cx: &mut Context<Self>) {
        self.relayout(f);
        self.after_scroll(cx);
    }
}

impl Painter<'_> {
    /// A gap row: `⋯ N unchanged lines` and its expanders. A run longer than
    /// [`EXPAND_STEP`] offers "↑ 20" (when a hunk follows it) and "↓ 20" (when
    /// one precedes it); every run offers "Expand all".
    pub(crate) fn gap_row(&mut self, f: u32, row: BodyRow, y: f32, h: f32) {
        let BodyRow::Gap {
            id, old_start, len, ..
        } = row
        else {
            return;
        };
        let (x, width) = self.inner_x_w();
        self.quad(FULL, x, y, width, h, self.theme.canvas);
        let s = if len == 1 { "" } else { "s" };
        let right = self.label_at(f, &format!("⋯ {len} unchanged line{s}"), y, h);
        // While the file reloads there is no diff to expand against.
        let Some(old_len) = self.materialized(f).map(|m| m.diff.old.len()) else {
            return;
        };
        let run = old_start..old_start + len;
        let a = self.geometry.advance;
        let mut x = right + 2.0 * a;
        let mut expander = |painter: &mut Self, by: ExpandBy, text: &str| {
            let action = ControlAction::Expand {
                file_idx: f,
                gap: id,
                by,
            };
            x = painter.link(action, text, x, y, h) + 0.5 * a;
            if let Some(control) = painter.frame.controls.last_mut() {
                control.run = Some(run.clone());
            }
        };
        if len > EXPAND_STEP {
            if run.end < old_len {
                expander(self, ExpandBy::Up(EXPAND_STEP), "↑ 20");
            }
            if run.start > 0 {
                expander(self, ExpandBy::Down(EXPAND_STEP), "↓ 20");
            }
        }
        expander(self, ExpandBy::All, "Expand all");
    }
}
