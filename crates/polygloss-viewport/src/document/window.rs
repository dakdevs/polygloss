//! The visible range, the materialization window and LRU-by-bytes eviction
//! (design §12.4 "Window").

use std::ops::Range;

use super::Document;
use super::file_state::{Body, FileState};

/// Materialize files within this many screens above and below the viewport
/// (**Provisional**, design §12.4).
pub const DEFAULT_WINDOW_SCREENS: f32 = 2.0;

/// Materialized data above this many bytes is evicted, farthest file first
/// (**Provisional**, design §12.4).
pub const DEFAULT_EVICTION_BUDGET_BYTES: usize = 256 * 1024 * 1024;

impl Document {
    /// Files intersecting the viewport `[scroll_top, scroll_top + viewport_h)`.
    pub fn visible(&self, viewport_h: f32) -> Range<u32> {
        self.overlapping(self.scroll_top, self.scroll_top + f64::from(viewport_h))
    }

    /// Files intersecting the viewport extended by `screens` viewport heights
    /// above and below: the files to materialize.
    pub fn materialize_range(&self, viewport_h: f32, screens: f32) -> Range<u32> {
        let viewport_h = f64::from(viewport_h);
        let margin = f64::from(screens.max(0.0)) * viewport_h;
        let top = self.scroll_top - margin;
        let bottom = if viewport_h > 0.0 {
            self.scroll_top + viewport_h + margin
        } else {
            top
        };
        self.overlapping(top, bottom)
    }

    /// Heap bytes held by materialized files and their layouts.
    pub fn resident_bytes(&self) -> usize {
        self.entries.iter().map(|e| e.resident_bytes()).sum()
    }

    /// Evicts materialized files, farthest from the viewport first, until
    /// [`Document::resident_bytes`] is at most `budget_bytes`. Files in the
    /// viewport (see [`Document::set_viewport_height`]) are never evicted.
    /// Evicted files keep their exact height, so nothing moves. Returns the
    /// evicted files in eviction order.
    pub fn evict_over_budget(&mut self, budget_bytes: usize) -> Vec<u32> {
        self.evict_over_budget_keeping(budget_bytes, 0..0)
    }

    /// [`Document::evict_over_budget`] that also keeps the files in `keep`
    /// (the materialization window: evicting them would only load them
    /// again). The budget is soft: when the kept files alone exceed it, they
    /// stay.
    pub fn evict_over_budget_keeping(&mut self, budget_bytes: usize, keep: Range<u32>) -> Vec<u32> {
        let mut resident = self.resident_bytes();
        if resident <= budget_bytes {
            return Vec::new();
        }
        let visible = self.visible(self.viewport_h);
        let (top, bottom) = (
            self.scroll_top,
            self.scroll_top + f64::from(self.viewport_h),
        );
        let mut candidates: Vec<(f64, u32)> = (0..self.len())
            .filter(|f| {
                !visible.contains(f) && !keep.contains(f) && self.state(*f).is_materialized()
            })
            .map(|f| {
                let start = self.file_top(f);
                let end = start + f64::from(self.file_height(f));
                ((top - end).max(start - bottom).max(0.0), f)
            })
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut evicted = Vec::new();
        for (_, f) in candidates {
            if resident <= budget_bytes {
                break;
            }
            resident -= self.evict(f);
            evicted.push(f);
        }
        evicted
    }

    /// Drops file `idx`'s data and rows, keeping its exact height; returns the
    /// bytes freed. In-flight work for it becomes stale.
    fn evict(&mut self, idx: u32) -> usize {
        let entry = &mut self.entries[idx as usize];
        let freed = entry.resident_bytes();
        entry.state = FileState::Evicted;
        entry.generation += 1;
        if let Body::Laid(layout) = &entry.body {
            entry.body = Body::Explicit(layout.height() as f32);
        }
        freed
    }

    /// Files intersecting `[top, bottom)` (empty, at the file containing `top`,
    /// when `bottom <= top`).
    fn overlapping(&self, top: f64, bottom: f64) -> Range<u32> {
        if self.entries.is_empty() {
            return 0..0;
        }
        let (first, _) = self.heights.find(top);
        if bottom.is_nan() || bottom <= top.max(0.0) {
            return first as u32..first as u32;
        }
        let (mut last, _) = self.heights.find(bottom);
        // A file starting exactly at `bottom` is outside.
        if last > first && self.heights.prefix(last) >= bottom {
            last -= 1;
        }
        first as u32..last as u32 + 1
    }
}
