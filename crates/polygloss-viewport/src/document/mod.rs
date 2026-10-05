//! The viewport's document model (design §12.4), pure Rust and GPUI-free.
//!
//! A [`Document`] holds an entry for **every** file of the diff (metadata only
//! until materialized), a [`HeightIndex`] over their heights for O(log n)
//! offset ↔ file lookups, and the logical [`ScrollAnchor`]. Heights start as
//! estimates ([`Metrics::estimate_body`], refined by [`SizeHint`]s) and become
//! exact when the viewport hands over a [`FileLayout`]. Every height change
//! re-derives the pixel scroll offset from the anchor, so corrections above
//! the anchor never move what is on screen.
//!
//! Each file is a card on the canvas (design §11.6): its height is its lead
//! (the canvas above the card; for the first file the prelude, the host's
//! header card, and a gap), the header, the body and the card's bottom
//! padding, and the last file also holds the gap below its card. With a zero
//! gap and padding and no prelude this is the flat v1 layout.
//!
//! Files are laid out in display order ([`slots`]): "first" and "last" above
//! mean the first and last shown file, and hidden files have no height.
//! Category sections ([`sections`]) come last; a section's band is in the
//! lead of its first file, which keeps it while hidden, so "first" and
//! "last" also count a closed section's band.

mod anchor;
mod file_layout;
mod file_state;
mod height_index;
mod metrics;
mod placement;
mod sections;
mod slots;
mod window;

use std::sync::Arc;

use polygloss_diff::{FileChange, FileKind, Side};

pub use anchor::{BlockId, RowKey, ScrollAnchor};
pub use file_layout::{BodyRow, FileLayout};
pub use file_state::FileState;
pub use height_index::HeightIndex;
pub use metrics::{Metrics, SizeHint};
pub use placement::{BlockAnchor, PlacedBlock};
pub use sections::SectionFiles;
pub use slots::{ShownFiles, SlotRange};
pub use window::{DEFAULT_EVICTION_BUDGET_BYTES, DEFAULT_WINDOW_SCREENS};

use crate::materialize::MaterializedFile;
use file_state::{Body, FileEntry};
use placement::block_height;

/// All files of one diff, their heights and the scroll position.
#[derive(Debug, Clone)]
pub struct Document {
    files: Arc<Vec<FileChange>>,
    metrics: Metrics,
    entries: Vec<FileEntry>,
    /// File indices in display order (slot → file).
    order: Vec<u32>,
    /// Each file's display slot (file → slot).
    slots: Vec<u32>,
    /// The first and last slots that are shown or hold a band; `None` when
    /// every file is hidden and no section has a band.
    shown: Option<(u32, u32)>,
    /// File heights by slot: lead, header, body and card padding
    /// ([`Document::file_height`]).
    heights: HeightIndex,
    anchor: ScrollAnchor,
    /// Category sections, in display order ([`sections`]).
    sections: Vec<SectionFiles>,
    /// Each file's index in `sections` ([`sections::NO_SECTION`]: none);
    /// empty until sections are set.
    section_of: Vec<u32>,
    /// Height of the prelude above the first card, when there is one.
    prelude: Option<f32>,
    /// Pixel offset of the viewport top, derived from `anchor`.
    scroll_top: f64,
    /// `scroll_top` relative to the anchor file's top, kept for anchors whose
    /// row disappears in a relayout.
    anchor_y: f64,
    viewport_h: f32,
}

impl Document {
    /// A document over `files` with every height estimated from the file's
    /// kind and status (refine with [`Document::set_size_hint`]), scrolled to
    /// the top.
    pub fn new(files: Arc<Vec<FileChange>>, metrics: Metrics) -> Document {
        let entries: Vec<FileEntry> = files
            .iter()
            .map(|change| FileEntry {
                state: FileState::Estimated,
                generation: 0,
                collapsed: false,
                hint: None,
                body: Body::Estimated(metrics.estimate_body(change, None)),
                blocks: Vec::new(),
                block_sets: 0,
                hidden: false,
                band: false,
            })
            .collect();
        let n = files.len() as u32;
        let mut doc = Document {
            files,
            metrics,
            entries,
            order: (0..n).collect(),
            slots: (0..n).collect(),
            shown: n.checked_sub(1).map(|last| (0, last)),
            heights: HeightIndex::default(),
            anchor: ScrollAnchor::default(),
            sections: Vec::new(),
            section_of: Vec::new(),
            prelude: None,
            scroll_top: 0.0,
            anchor_y: 0.0,
            viewport_h: 0.0,
        };
        doc.reindex();
        doc
    }

    pub fn files(&self) -> &Arc<Vec<FileChange>> {
        &self.files
    }

    /// Number of files.
    pub fn len(&self) -> u32 {
        self.entries.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// Replaces the geometry (e.g. a split ↔ unified toggle or a font change):
    /// drops every row layout and exact height, re-estimates every file and
    /// keeps the anchor.
    pub fn set_metrics(&mut self, metrics: Metrics) {
        for (entry, change) in self.entries.iter_mut().zip(self.files.iter()) {
            entry.body = Body::Estimated(metrics.estimate_body(change, entry.hint));
        }
        self.metrics = metrics;
        self.reindex();
        self.rebase();
    }

    /// Height of the prelude above the first card (the host's header card),
    /// or `None` without one.
    pub fn prelude_height(&self) -> Option<f32> {
        self.prelude
    }

    /// Sets the prelude's height (`None`: no prelude). It is the first shown
    /// file's lead, with a gap below it. The anchor stays put, so an anchor at
    /// the top of the document keeps the prelude at the top while it grows.
    pub fn set_prelude_height(&mut self, h: Option<f32>) {
        let h = h.map(|h| if h.is_finite() { h.max(0.0) } else { 0.0 });
        if self.prelude == h {
            return;
        }
        self.prelude = h;
        if !self.entries.is_empty() {
            self.refresh(self.file_at(self.top_slot()));
        }
    }

    /// The canvas above file `idx`'s card: [`Metrics::card_gap`], except for
    /// the first shown file, whose lead is the prelude and a gap when there
    /// is a prelude, else nothing, and hidden files, which have none. A
    /// section's first file adds its band ([`Metrics::band_height`]) below
    /// that, shown or hidden.
    pub fn lead(&self, idx: u32) -> f32 {
        let band = if self.has_band(idx) {
            self.metrics.band_height
        } else {
            0.0
        };
        let above = if self.slot(idx) == self.top_slot() {
            self.prelude.map_or(0.0, |p| p + self.metrics.card_gap)
        } else if self.is_hidden(idx) && band == 0.0 {
            0.0
        } else {
            self.metrics.card_gap
        };
        above + band
    }

    /// Top of file `idx`'s header (its card): [`Document::file_top`] plus its
    /// [`Document::lead`].
    pub fn header_top(&self, idx: u32) -> f64 {
        self.file_top(idx) + f64::from(self.lead(idx))
    }

    /// Top of file `idx`'s body: [`Document::header_top`] plus the header.
    pub fn body_top(&self, idx: u32) -> f64 {
        self.header_top(idx) + f64::from(self.metrics.header_height)
    }

    /// Height of file `idx`'s body (rows and blocks, without the card's
    /// padding); 0 when collapsed.
    pub fn body_height(&self, idx: u32) -> f32 {
        self.entries
            .get(idx as usize)
            .map_or(0.0, FileEntry::body_height)
    }

    /// Bottom of file `idx`'s card: the body's bottom plus the card's
    /// padding (none under an empty or collapsed body).
    pub fn card_bottom(&self, idx: u32) -> f64 {
        self.body_top(idx) + f64::from(self.padded(self.body_height(idx)))
    }

    /// A body of height `body` with the card's padding below it, if it is not
    /// empty.
    fn padded(&self, body: f32) -> f32 {
        if body > 0.0 {
            body + self.metrics.card_pad_bottom
        } else {
            0.0
        }
    }

    /// File `idx`'s height in the document: its lead, header, body and card
    /// padding, plus the gap below the last card (or band); only its lead
    /// (and that gap) when hidden.
    fn entry_height(&self, idx: u32) -> f32 {
        let tail = if self.last_shown_slot() == Some(self.slot(idx)) {
            self.metrics.card_gap
        } else {
            0.0
        };
        if self.is_hidden(idx) {
            return self.lead(idx) + tail;
        }
        let body = self.padded(self.entries[idx as usize].body_height());
        self.lead(idx) + self.metrics.header_height + body + tail
    }

    /// Rebuilds the height index from every entry, in display order.
    fn reindex(&mut self) {
        let heights: Vec<f32> = self.order.iter().map(|&f| self.entry_height(f)).collect();
        self.heights = HeightIndex::new(&heights);
    }

    /// The viewport height, used to clamp scrolling at the bottom and to find
    /// the visible files that eviction must keep.
    pub fn set_viewport_height(&mut self, h: f32) {
        self.viewport_h = if h.is_finite() && h > 0.0 { h } else { 0.0 };
        self.rebase();
    }

    pub fn viewport_height(&self) -> f32 {
        self.viewport_h
    }

    /// Height of the whole document.
    pub fn total_height(&self) -> f64 {
        self.heights.total()
    }

    /// Pixel offset of the viewport top.
    pub fn scroll_top(&self) -> f64 {
        self.scroll_top
    }

    /// The largest `scroll_top`: the last pixel row at the viewport bottom.
    pub fn max_scroll(&self) -> f64 {
        (self.heights.total() - f64::from(self.viewport_h)).max(0.0)
    }

    /// Top of file `idx`: the top of its lead.
    pub fn file_top(&self, idx: u32) -> f64 {
        self.heights.prefix(self.slot(idx) as usize)
    }

    /// Height of file `idx`: lead, header, body and card padding (and the gap
    /// below the last card); a hidden file's lead.
    pub fn file_height(&self, idx: u32) -> f32 {
        self.heights.get(self.slot(idx) as usize)
    }

    /// The file containing document offset `offset` and the offset within it;
    /// past the end, the last file that has a height (files without one, such
    /// as hidden files, never contain an offset; a closed section's first
    /// file has its band's).
    pub fn file_at_offset(&self, offset: f64) -> (u32, f64) {
        let (mut slot, mut y) = self.heights.find(offset);
        // `find` gives its last slot past the end, whatever its height.
        if !self.heights.is_empty()
            && self.heights.get(slot) == 0.0
            && let Some(last) = self.heights.last_before(self.heights.total())
        {
            (slot, y) = (last, offset - self.heights.prefix(last));
        }
        (self.order.get(slot).copied().unwrap_or(0), y)
    }

    pub fn anchor(&self) -> &ScrollAnchor {
        &self.anchor
    }

    /// Scrolls by `dy` pixels (positive = down), clamped to the document, and
    /// re-derives the anchor from the new position. Inside a file that has no
    /// rows yet, a pending target from [`Document::scroll_to`] (a line, gap or
    /// block) is kept and only its offset moves, so the target still resolves
    /// exactly once the file is laid out. That holds anywhere below the top
    /// of the file's header, where the viewport pins the header over its
    /// body; at or above it the anchor is the header or the lead.
    pub fn scroll_by(&mut self, dy: f32) {
        if self.entries.is_empty() || dy.is_nan() {
            return;
        }
        let to = (self.scroll_top + f64::from(dy)).clamp(0.0, self.max_scroll());
        if to == self.scroll_top {
            return;
        }
        let (f, y) = self.file_at_offset(to);
        let pending =
            f == self.anchor.file_idx && self.pending_target(f) && y > f64::from(self.lead(f));
        let key_y = if pending {
            self.key_offset(f, self.anchor.row)
        } else {
            None
        };
        match key_y {
            Some(key_y) => self.anchor.offset_px = (y - key_y) as f32,
            None => self.anchor = self.anchor_at(to),
        }
        self.scroll_top = to;
        self.anchor_y = to - self.file_top(self.anchor.file_idx);
    }

    /// Puts row `row` of file `file_idx` at the viewport top (clamped to the
    /// document).
    pub fn scroll_to(&mut self, file_idx: u32, row: RowKey) {
        self.scroll_to_anchor(ScrollAnchor {
            file_idx,
            row,
            offset_px: 0.0,
        });
    }

    /// Restores a scroll position (view state, or a target above the row). A
    /// position in a hidden file is its section's band (the top of the
    /// section's first file), or the top of where it is: `(file, Lead, 0)`.
    pub fn scroll_to_anchor(&mut self, anchor: ScrollAnchor) {
        if self.entries.is_empty() {
            return;
        }
        self.anchor = ScrollAnchor {
            file_idx: anchor.file_idx.min(self.len() - 1),
            offset_px: if anchor.offset_px.is_finite() {
                anchor.offset_px
            } else {
                0.0
            },
            ..anchor
        };
        // A collapsed file shows only its header; a row key kept there would
        // jump to that row when the file is expanded.
        if self.is_collapsed(self.anchor.file_idx) && !self.anchor.row.is_above_body() {
            self.anchor.row = RowKey::Header;
            self.anchor.offset_px = 0.0;
        }
        if self.is_hidden(self.anchor.file_idx) {
            self.anchor = self.hidden_anchor(self.anchor.file_idx);
        }
        // A key that does not resolve (a block or gap that is gone, an old line
        // of an added file) falls back to the top of the body.
        let body = self.lead(self.anchor.file_idx) + self.metrics.header_height;
        self.anchor_y = f64::from(body) + f64::from(self.anchor.offset_px);
        self.rebase();
    }

    /// Offset of row `key` from the top of file `idx` (the top of its lead):
    /// exact when the file is laid out, estimated otherwise. `Lead` is 0 and
    /// `Header` the lead's height; every other key of a collapsed file
    /// resolves to its header.
    pub fn key_offset(&self, idx: u32, key: RowKey) -> Option<f64> {
        let entry = self.entries.get(idx as usize)?;
        if key == RowKey::Lead {
            return Some(0.0);
        }
        let lead = f64::from(self.lead(idx));
        if key == RowKey::Header || entry.collapsed {
            return Some(lead);
        }
        let header = lead + f64::from(self.metrics.header_height);
        match &entry.body {
            Body::Laid(layout) => layout.find(key).map(|r| header + layout.row_top(r)),
            Body::Estimated(body) | Body::Explicit(body) => {
                let row_h = f64::from(self.metrics.row_height);
                // Lines are spread evenly over an estimated body.
                let line_y =
                    |line: u32| (f64::from(line) * row_h).min((f64::from(*body) - row_h).max(0.0));
                let y = match key {
                    RowKey::Line { line, .. } => line_y(line),
                    // A block sits below its line.
                    RowKey::Block(id) => match entry.blocks.iter().find(|b| b.id == id) {
                        Some(PlacedBlock {
                            anchor: BlockAnchor::Line { line, .. },
                            ..
                        }) => line_y(*line) + row_h,
                        _ => 0.0,
                    },
                    _ => 0.0,
                };
                Some(header + y)
            }
        }
    }

    /// Sets file `idx`'s exact header and body height (the lead and the card's
    /// padding are not part of it) for a body that has no rows; the file's
    /// blocks add to it. Content on screen does not move: the anchor stays
    /// put.
    pub fn set_file_height(&mut self, idx: u32, h: f32) {
        let body = if h.is_finite() {
            (h - self.metrics.header_height).max(0.0)
        } else {
            0.0
        };
        let entry = &mut self.entries[idx as usize];
        entry.body = Body::Explicit(body + entry.blocks_height());
        self.refresh(idx);
    }

    /// Hands over file `idx`'s exact rows. The file's blocks (see
    /// [`Document::set_blocks`]) are placed in them, replacing any block rows
    /// `layout` has. The anchor stays put; if it was in this file and its row
    /// no longer exists, the same pixel stays at the top.
    pub fn set_file_layout(&mut self, idx: u32, layout: FileLayout) {
        let entry = &mut self.entries[idx as usize];
        let layout = if entry.blocks.is_empty() && layout.block_rows().is_empty() {
            layout
        } else {
            layout.with_blocks(&entry.blocks)
        };
        entry.body = Body::Laid(layout);
        self.refresh(idx);
    }

    /// Replaces file `idx`'s host blocks (design §12.4 "Blocks"). Only this
    /// file is laid out again: a laid-out body gets them as rows, an estimated
    /// or row-less one adds their heights. Duplicate ids keep the first. The
    /// anchor stays put, so blocks above it never move what is on screen.
    pub fn set_blocks(&mut self, idx: u32, mut blocks: Vec<PlacedBlock>) {
        let mut seen = std::collections::HashSet::with_capacity(blocks.len());
        blocks.retain(|b| seen.insert(b.id));
        for b in &mut blocks {
            b.height = block_height(b.height);
        }
        let entry = &mut self.entries[idx as usize];
        let old = entry.blocks_height();
        entry.blocks = blocks;
        entry.block_sets += 1;
        let new = entry.blocks_height();
        match &mut entry.body {
            Body::Laid(layout) => *layout = layout.with_blocks(&entry.blocks),
            Body::Explicit(h) => *h = (*h - old + new).max(0.0),
            Body::Estimated(_) => {}
        }
        self.refresh(idx);
    }

    /// How many times file `idx`'s blocks were replaced
    /// ([`Document::set_blocks`], which lays that file out again): hosts'
    /// tests check that an update re-lays out only the files it should.
    pub fn block_sets(&self, idx: u32) -> u32 {
        self.entries.get(idx as usize).map_or(0, |e| e.block_sets)
    }

    /// File `idx`'s host blocks with their current heights, in the host's
    /// order.
    pub fn blocks(&self, idx: u32) -> &[PlacedBlock] {
        self.entries
            .get(idx as usize)
            .map_or(&[][..], |e| &e.blocks)
    }

    /// Sets block `id`'s height in file `idx` (measured by the viewport). It
    /// is kept by later relayouts. The anchor stays put. Returns whether the
    /// file has that block.
    pub fn set_block_height(&mut self, idx: u32, id: BlockId, h: f32) -> bool {
        let h = block_height(h);
        let Some(entry) = self.entries.get_mut(idx as usize) else {
            return false;
        };
        let Some(block) = entry.blocks.iter_mut().find(|b| b.id == id) else {
            return false;
        };
        let old = std::mem::replace(&mut block.height, h);
        match &mut entry.body {
            Body::Laid(layout) => {
                if let Some(row) = layout.find(RowKey::Block(id)) {
                    // A pair's row is as tall as its taller block.
                    let row_h = layout.rows()[row]
                        .block_ids()
                        .into_iter()
                        .flatten()
                        .filter_map(|b| entry.blocks.iter().find(|p| p.id == b))
                        .map(|b| b.height)
                        .fold(h, f32::max);
                    layout.set_row_height(row, row_h);
                }
            }
            Body::Explicit(body) => *body = (*body - old + h).max(0.0),
            Body::Estimated(_) => {}
        }
        self.refresh(idx);
        true
    }

    /// File `idx`'s rows, when laid out.
    pub fn file_layout(&self, idx: u32) -> Option<&FileLayout> {
        self.entries.get(idx as usize)?.layout()
    }

    /// Changes one row's height in a laid-out file (a measured block, a wrapped
    /// line); the anchor stays put. Ignored when the file has no layout. A
    /// single block row's new height is kept by later relayouts (a pair's
    /// row is re-derived from its blocks' heights: set those with
    /// [`Document::set_block_height`]).
    pub fn set_row_height(&mut self, idx: u32, row: u32, h: f32) {
        let entry = &mut self.entries[idx as usize];
        if let Body::Laid(layout) = &mut entry.body
            && (row as usize) < layout.len()
        {
            if let BodyRow::Block(id) = layout.rows()[row as usize]
                && let Some(block) = entry.blocks.iter_mut().find(|b| b.id == id)
            {
                block.height = block_height(h);
            }
            layout.set_row_height(row as usize, h);
            self.refresh(idx);
        }
    }

    /// Refines the estimate of file `idx`. Ignored once the height is exact,
    /// and blob sizes are ignored once counts are known.
    pub fn set_size_hint(&mut self, idx: u32, hint: SizeHint) {
        let entry = &mut self.entries[idx as usize];
        if matches!(
            (entry.hint, hint),
            (Some(SizeHint::Counts { .. }), SizeHint::BlobSizes { .. })
        ) {
            return;
        }
        entry.hint = Some(hint);
        if let Body::Estimated(_) = entry.body {
            let estimate = self
                .metrics
                .estimate_body(&self.files[idx as usize], entry.hint);
            entry.body = Body::Estimated(estimate);
            self.refresh(idx);
        }
    }

    /// The latest size hint of file `idx` (kept once the height is exact, so
    /// headers can still show counts).
    pub fn size_hint(&self, idx: u32) -> Option<SizeHint> {
        self.entries.get(idx as usize)?.hint
    }

    /// The same files with new metadata (a Generated verdict,
    /// [`crate::DiffViewport::set_generated`]): the files in `changed` go back
    /// to estimates from their new metadata (the view lays them out again);
    /// every other file keeps its height and rows. The anchor stays put.
    pub fn replace_files(&mut self, files: Arc<Vec<FileChange>>, changed: &[u32]) {
        debug_assert_eq!(files.len(), self.files.len());
        self.files = files;
        for &f in changed {
            let Some(entry) = self.entries.get_mut(f as usize) else {
                continue;
            };
            let estimate = self
                .metrics
                .estimate_body(&self.files[f as usize], entry.hint);
            entry.body = Body::Estimated(estimate);
            self.refresh(f);
        }
    }

    /// Changes file `idx`'s kind (binary content found when its blobs were
    /// first read, design §6.4) and re-estimates its body unless its height is
    /// exact. The file list is copied on the first change if it is shared.
    pub fn set_kind(&mut self, idx: u32, kind: FileKind) {
        let i = idx as usize;
        if self.files[i].kind == kind {
            return;
        }
        Arc::make_mut(&mut self.files)[i].kind = kind;
        if let Body::Estimated(_) = self.entries[i].body {
            let estimate = self
                .metrics
                .estimate_body(&self.files[i], self.entries[i].hint);
            self.entries[i].body = Body::Estimated(estimate);
            self.refresh(idx);
        }
    }

    /// Whether file `idx`'s height is exact (laid out or set), not estimated.
    pub fn is_exact(&self, idx: u32) -> bool {
        !matches!(self.entries[idx as usize].body, Body::Estimated(_))
    }

    /// Collapses file `idx` to its header, or expands it. Collapsing the file
    /// the anchor is in moves an anchor in its body to its header.
    pub fn set_collapsed(&mut self, idx: u32, collapsed: bool) {
        let entry = &mut self.entries[idx as usize];
        if entry.collapsed == collapsed {
            return;
        }
        entry.collapsed = collapsed;
        if collapsed && self.anchor.file_idx == idx && !self.anchor.row.is_above_body() {
            self.anchor.row = RowKey::Header;
            self.anchor.offset_px = 0.0;
        }
        self.refresh(idx);
    }

    pub fn is_collapsed(&self, idx: u32) -> bool {
        self.entries[idx as usize].collapsed
    }

    pub fn state(&self, idx: u32) -> &FileState {
        &self.entries[idx as usize].state
    }

    /// The current generation of file `idx`.
    pub fn generation(&self, idx: u32) -> u64 {
        self.entries[idx as usize].generation
    }

    /// Starts (or restarts) loading file `idx`: bumps its generation, which
    /// makes every in-flight result for it stale, and returns the new one.
    pub fn begin_loading(&mut self, idx: u32) -> u64 {
        let entry = &mut self.entries[idx as usize];
        entry.generation += 1;
        entry.state = FileState::Loading {
            generation: entry.generation,
        };
        entry.generation
    }

    /// Stores a materialized file (first load, or a newer version such as one
    /// with tokens) if `generation` is still current. Returns whether it was
    /// accepted. Heights do not change until the viewport sets a layout.
    pub fn set_materialized(
        &mut self,
        idx: u32,
        generation: u64,
        file: Arc<MaterializedFile>,
    ) -> bool {
        let entry = &mut self.entries[idx as usize];
        let current = entry.generation == generation
            && matches!(
                entry.state,
                FileState::Loading { .. } | FileState::Materialized(_)
            );
        if current {
            entry.state = FileState::Materialized(file);
        }
        current
    }

    /// Marks a load as failed if `generation` is still current.
    pub fn set_failed(&mut self, idx: u32, generation: u64, message: String) -> bool {
        let entry = &mut self.entries[idx as usize];
        let current = entry.generation == generation
            && matches!(
                entry.state,
                FileState::Loading { .. } | FileState::Materialized(_)
            );
        if current {
            entry.state = FileState::Failed(message);
        }
        current
    }

    /// Abandons a load (the file scrolled away): the file goes back to
    /// `Evicted` if it was laid out before, else `Estimated`, and in-flight
    /// results become stale.
    pub fn cancel_loading(&mut self, idx: u32) {
        let entry = &mut self.entries[idx as usize];
        if let FileState::Loading { .. } = entry.state {
            entry.generation += 1;
            entry.state = match entry.body {
                Body::Estimated(_) => FileState::Estimated,
                Body::Explicit(_) | Body::Laid(_) => FileState::Evicted,
            };
        }
    }

    /// Re-reads file `idx`'s height into the index and keeps the anchor.
    fn refresh(&mut self, idx: u32) {
        let h = self.entry_height(idx);
        self.heights.set(self.slot(idx) as usize, h);
        self.rebase();
    }

    /// Re-derives `scroll_top` from the anchor after heights changed. The
    /// anchor is kept while its row resolves inside its file; otherwise the
    /// same pixel of the anchor file (`anchor_y`) stays at the top and a new
    /// anchor is taken there. Clamping at the document's end never replaces
    /// the anchor, so a jump near the end still lands once the heights
    /// before it are known.
    fn rebase(&mut self) {
        if self.entries.is_empty() {
            self.anchor = ScrollAnchor::default();
            (self.scroll_top, self.anchor_y) = (0.0, 0.0);
            return;
        }
        let file = self.anchor.file_idx;
        let file_h = f64::from(self.file_height(file));
        let resolved = self
            .key_offset(file, self.anchor.row)
            .map(|y| y + f64::from(self.anchor.offset_px))
            .filter(|&y| y >= 0.0 && (y < file_h || y == 0.0));
        let desired = self.file_top(file) + resolved.unwrap_or(self.anchor_y);
        let desired = desired.clamp(0.0, self.heights.total());
        if resolved.is_none() {
            self.anchor = self.anchor_at(desired);
        }
        self.anchor_y = desired - self.file_top(self.anchor.file_idx);
        self.scroll_top = desired.clamp(0.0, self.max_scroll());
    }

    /// The anchor for document offset `offset`: the top of the document at
    /// (or above) 0, else the lead, the header, the row at that pixel, or the
    /// placeholder body of a file without rows.
    fn anchor_at(&self, offset: f64) -> ScrollAnchor {
        if offset <= 0.0 {
            return self.top_anchor();
        }
        let (file_idx, y) = self.file_at_offset(offset);
        let lead = f64::from(self.lead(file_idx));
        let body = lead + f64::from(self.metrics.header_height);
        let entry = &self.entries[file_idx as usize];
        let (row, offset) = if y < lead {
            (RowKey::Lead, y)
        } else if y < body || entry.collapsed {
            (RowKey::Header, y - lead)
        } else {
            match &entry.body {
                Body::Laid(layout) => layout
                    .key_at(y - body)
                    .unwrap_or((RowKey::Header, y - lead)),
                Body::Estimated(_) | Body::Explicit(_) => (RowKey::Placeholder, y - body),
            }
        };
        ScrollAnchor {
            file_idx,
            row,
            offset_px: offset as f32,
        }
    }

    /// The first pixel row below the header pinned at the viewport's top
    /// (`scroll_top` plus a header's height) as `(file, y)`, `y` relative to
    /// that file's body top (negative in its lead or header). In a card's
    /// padding, where nothing of that card's body shows any more, or in a
    /// hidden file's lead, it is the next shown file's, above its body.
    pub(crate) fn below_header(&self) -> (u32, f64) {
        let header = f64::from(self.metrics.header_height);
        let at = self.scroll_top + header;
        let (f, _) = self.file_at_offset(at);
        let y = at - self.body_top(f);
        if (self.is_hidden(f) || y >= f64::from(self.body_height(f)))
            && let Some(next) = self.next_shown(f)
        {
            return (next, -1.0);
        }
        (f, y)
    }

    /// The first line shown below the header pinned at the top, as
    /// `(file_idx, side, line)` with a 0-based line: the new side where a
    /// row has it, else the old one; a gap row counts as its first hidden
    /// (old) line. Restoring it with a line target puts it right below the
    /// header again, so the same line is first at any width, height or font
    /// size. In a lead or a header it is the first line of that file's body;
    /// a collapsed file (or one without lines) gives its first line, which
    /// restores to its header at the top; a file not laid out yet gives the
    /// line a pending scroll target aims at, else an estimate. `None` for an
    /// empty diff.
    pub fn top_line(&self) -> Option<(u32, Side, u32)> {
        if self.is_empty() {
            return None;
        }
        let (f, y) = self.below_header();
        let side = if self.files[f as usize].new_path.is_none() {
            Side::Old
        } else {
            Side::New
        };
        if self.is_collapsed(f) {
            return Some((f, side, 0));
        }
        let Some(layout) = self.file_layout(f) else {
            if self.anchor.file_idx == f
                && let RowKey::Line { side, line } = self.anchor.row
            {
                return Some((f, side, line));
            }
            let row = f64::from(self.metrics.row_height).max(1.0);
            return Some((f, side, (y.max(0.0) / row) as u32));
        };
        let rows = layout.rows();
        if rows.is_empty() {
            return Some((f, side, 0));
        }
        let first = if y >= 0.0 {
            layout.row_at(y).0.min(rows.len() - 1)
        } else {
            0
        };
        let line_of = |row: &BodyRow| match *row {
            BodyRow::Line { new: Some(l), .. } => Some((Side::New, l)),
            BodyRow::Line { old: Some(l), .. } => Some((Side::Old, l)),
            BodyRow::Gap { old_start, .. } => Some((Side::Old, old_start)),
            _ => None,
        };
        // Blocks and markers have no line: the next line down, else the one
        // above them (the end of the file).
        let (side, line) = rows[first..]
            .iter()
            .find_map(line_of)
            .or_else(|| rows[..first].iter().rev().find_map(line_of))
            .unwrap_or((side, 0));
        Some((f, side, line))
    }

    /// Whether the anchor is a target inside file `idx` that cannot resolve
    /// exactly yet: a line, gap or block of an expanded file without rows.
    fn pending_target(&self, idx: u32) -> bool {
        let entry = &self.entries[idx as usize];
        !entry.collapsed
            && entry.layout().is_none()
            && matches!(
                self.anchor.row,
                RowKey::Line { .. } | RowKey::Gap(_) | RowKey::Block(_)
            )
    }
}
