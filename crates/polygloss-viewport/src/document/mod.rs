//! The viewport's document model (design §12.4), pure Rust and GPUI-free.
//!
//! A [`Document`] holds an entry for **every** file of the diff (metadata only
//! until materialized), a [`HeightIndex`] over their heights for O(log n)
//! offset ↔ file lookups, and the logical [`ScrollAnchor`]. Heights start as
//! estimates ([`Metrics::estimate_body`], refined by [`SizeHint`]s) and become
//! exact when the viewport hands over a [`FileLayout`]. Every height change
//! re-derives the pixel scroll offset from the anchor, so corrections above
//! the anchor never move what is on screen.

mod anchor;
mod file_layout;
mod file_state;
mod height_index;
mod metrics;
mod placement;
mod window;

use std::sync::Arc;

use polygloss_diff::{FileChange, FileKind};

pub use anchor::{BlockId, RowKey, ScrollAnchor};
pub use file_layout::{BodyRow, FileLayout};
pub use file_state::FileState;
pub use height_index::HeightIndex;
pub use metrics::{Metrics, SizeHint};
pub use placement::{BlockAnchor, PlacedBlock};
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
    /// File heights (header plus body, or the header alone when collapsed).
    heights: HeightIndex,
    anchor: ScrollAnchor,
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
            })
            .collect();
        let heights: Vec<f32> = entries
            .iter()
            .map(|e| e.height(metrics.header_height))
            .collect();
        Document {
            files,
            metrics,
            entries,
            heights: HeightIndex::new(&heights),
            anchor: ScrollAnchor::default(),
            scroll_top: 0.0,
            anchor_y: 0.0,
            viewport_h: 0.0,
        }
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
        let heights: Vec<f32> = self
            .entries
            .iter()
            .map(|e| e.height(metrics.header_height))
            .collect();
        self.heights = HeightIndex::new(&heights);
        self.metrics = metrics;
        self.rebase();
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

    /// Top of file `idx`.
    pub fn file_top(&self, idx: u32) -> f64 {
        self.heights.prefix(idx as usize)
    }

    /// Height of file `idx`, header included.
    pub fn file_height(&self, idx: u32) -> f32 {
        self.heights.get(idx as usize)
    }

    /// The file containing document offset `offset` and the offset within it.
    pub fn file_at(&self, offset: f64) -> (u32, f64) {
        let (idx, y) = self.heights.find(offset);
        (idx as u32, y)
    }

    pub fn anchor(&self) -> &ScrollAnchor {
        &self.anchor
    }

    /// Scrolls by `dy` pixels (positive = down), clamped to the document, and
    /// re-derives the anchor from the new position. Inside a file that has no
    /// rows yet, a pending target from [`Document::scroll_to`] (a line, gap or
    /// block) is kept and only its offset moves, so the target still resolves
    /// exactly once the file is laid out. That holds anywhere below the
    /// file's top edge, where the viewport pins the file's header over its
    /// body; at the top edge the anchor is the header.
    pub fn scroll_by(&mut self, dy: f32) {
        if self.entries.is_empty() || dy.is_nan() {
            return;
        }
        let to = (self.scroll_top + f64::from(dy)).clamp(0.0, self.max_scroll());
        if to == self.scroll_top {
            return;
        }
        let (f, y) = self.heights.find(to);
        let pending = f as u32 == self.anchor.file_idx && self.pending_target(f) && y > 0.0;
        let key_y = if pending {
            self.key_offset(f as u32, self.anchor.row)
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

    /// Restores a scroll position (view state, or a target above the row).
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
        if self.is_collapsed(self.anchor.file_idx) && self.anchor.row != RowKey::Header {
            self.anchor.row = RowKey::Header;
            self.anchor.offset_px = 0.0;
        }
        // A key that does not resolve (a block or gap that is gone, an old line
        // of an added file) falls back to the top of the body.
        self.anchor_y = f64::from(self.metrics.header_height) + f64::from(self.anchor.offset_px);
        self.rebase();
    }

    /// Offset of row `key` from the top of file `idx`: exact when the file is
    /// laid out, estimated otherwise. Every key of a collapsed file resolves
    /// to its header (0).
    pub fn key_offset(&self, idx: u32, key: RowKey) -> Option<f64> {
        let entry = self.entries.get(idx as usize)?;
        if key == RowKey::Header || entry.collapsed {
            return Some(0.0);
        }
        let header = f64::from(self.metrics.header_height);
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

    /// Sets file `idx`'s exact height (header included) for a body that has no
    /// rows; the file's blocks add to it. Content on screen does not move: the
    /// anchor stays put.
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
    /// the anchor is in moves the anchor to its header.
    pub fn set_collapsed(&mut self, idx: u32, collapsed: bool) {
        let entry = &mut self.entries[idx as usize];
        if entry.collapsed == collapsed {
            return;
        }
        entry.collapsed = collapsed;
        if collapsed && self.anchor.file_idx == idx && self.anchor.row != RowKey::Header {
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
        let h = self.entries[idx as usize].height(self.metrics.header_height);
        self.heights.set(idx as usize, h);
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

    /// The anchor for document offset `offset`: the header, the row at that
    /// pixel, or the placeholder body of a file without rows.
    fn anchor_at(&self, offset: f64) -> ScrollAnchor {
        let (idx, y) = self.heights.find(offset);
        let file_idx = idx as u32;
        let header = f64::from(self.metrics.header_height);
        let entry = &self.entries[idx];
        let (row, offset) = if y < header || entry.collapsed {
            (RowKey::Header, y)
        } else {
            match &entry.body {
                Body::Laid(layout) => layout.key_at(y - header).unwrap_or((RowKey::Header, y)),
                Body::Estimated(_) | Body::Explicit(_) => (RowKey::Placeholder, y - header),
            }
        };
        ScrollAnchor {
            file_idx,
            row,
            offset_px: offset as f32,
        }
    }

    /// Whether the anchor is a target inside file `idx` that cannot resolve
    /// exactly yet: a line, gap or block of an expanded file without rows.
    fn pending_target(&self, idx: usize) -> bool {
        let entry = &self.entries[idx];
        !entry.collapsed
            && entry.layout().is_none()
            && matches!(
                self.anchor.row,
                RowKey::Line { .. } | RowKey::Gap(_) | RowKey::Block(_)
            )
    }
}
