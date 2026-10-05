//! Display slots (design §11.6 "Sections", §11.15): the order files are
//! shown in, and the files that are hidden. A file's slot is its position in
//! display order and the height index is by slot; every public API stays
//! keyed by `file_idx` (git order). A hidden file is only its lead (nothing
//! until T6.17 gives a section band one): it is never laid out, painted,
//! loaded or walked, and walks step over it by offset, so hidden files cost
//! nothing per frame.

use super::Document;
use super::anchor::{RowKey, ScrollAnchor};

/// Display slots `start..end`. Deliberately not an iterator (`for f in
/// range` would walk slots as if they were files): walk the files it shows
/// with [`Document::shown_files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRange {
    pub start: u32,
    pub end: u32,
}

impl SlotRange {
    pub fn contains(&self, slot: u32) -> bool {
        (self.start..self.end).contains(&slot)
    }
}

/// The shown files of a [`SlotRange`], in display order
/// ([`Document::shown_files`]).
#[derive(Debug, Clone)]
pub struct ShownFiles<'a> {
    doc: &'a Document,
    /// Where the next look starts: the top of the slot after the last one
    /// looked at.
    from: f64,
    end: u32,
    visits: u32,
}

impl ShownFiles<'_> {
    /// Slots looked at so far: one per shown file, one per hidden file that
    /// has a height, and the one past the range that ends the walk. Hidden
    /// files without a height are never looked at.
    pub fn visits(&self) -> u32 {
        self.visits
    }
}

impl Iterator for ShownFiles<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let heights = &self.doc.heights;
        while self.from < heights.total() {
            self.visits += 1;
            // The first slot below `from` that has a height.
            let (slot, _) = heights.find(self.from);
            if slot as u32 >= self.end {
                break;
            }
            self.from = heights.prefix(slot + 1);
            let f = self.doc.order[slot];
            if !self.doc.is_hidden(f) {
                return Some(f);
            }
        }
        self.from = f64::INFINITY;
        None
    }
}

impl Document {
    /// File `file_idx`'s display slot. A file past the end is its own slot.
    pub fn slot(&self, file_idx: u32) -> u32 {
        self.slots
            .get(file_idx as usize)
            .copied()
            .unwrap_or(file_idx)
    }

    /// The file in display slot `slot`. Panics if `slot` is out of range.
    pub fn file_at(&self, slot: u32) -> u32 {
        self.order[slot as usize]
    }

    /// Every file index, in display order.
    pub fn display_order(&self) -> &[u32] {
        &self.order
    }

    pub fn is_hidden(&self, file_idx: u32) -> bool {
        self.entries
            .get(file_idx as usize)
            .is_some_and(|e| e.hidden)
    }

    /// The shown files of `range`, in display order. It steps by offset, so
    /// hidden files (which have no height) cost nothing.
    pub fn shown_files(&self, range: SlotRange) -> ShownFiles<'_> {
        ShownFiles {
            doc: self,
            from: self.heights.prefix(range.start as usize),
            end: range.end,
            visits: 0,
        }
    }

    /// Whether file `file_idx` is shown in `range`: in one of its slots and
    /// not hidden.
    pub fn contains_file(&self, range: SlotRange, file_idx: u32) -> bool {
        range.contains(self.slot(file_idx)) && !self.is_hidden(file_idx)
    }

    /// Shows the files in `order`, a permutation of `0..len()` (anything else
    /// is ignored). The anchor stays put; an anchor at the top of the
    /// document ([`Document::top_anchor`]), or further into the first shown
    /// file's lead (the prelude), stays there.
    pub fn set_order(&mut self, order: Vec<u32>) {
        let n = self.len() as usize;
        let mut slots = vec![u32::MAX; n];
        let permutation = order.len() == n
            && order.iter().enumerate().all(|(slot, &f)| {
                slots
                    .get_mut(f as usize)
                    .is_some_and(|s| std::mem::replace(s, slot as u32) == u32::MAX)
            });
        if !permutation {
            debug_assert!(false, "display order is not a permutation of 0..{n}");
            return;
        }
        let top = self.offset_in_top_lead();
        self.order = order;
        self.slots = slots;
        self.relayout_slots(top);
    }

    /// Hides `files` (or shows them again): a hidden file is only its lead
    /// and is never laid out, painted, loaded or walked. An anchor at the top
    /// of the document (or in the prelude) stays there; an anchor in a file
    /// that is hidden now moves to the top of where that file was, `(file,
    /// Lead, 0)`.
    pub fn set_hidden(&mut self, files: &[u32], hidden: bool) {
        let top = self.offset_in_top_lead();
        let mut changed = false;
        for &f in files {
            if let Some(entry) = self.entries.get_mut(f as usize)
                && entry.hidden != hidden
            {
                entry.hidden = hidden;
                changed = true;
            }
        }
        if !changed {
            return;
        }
        if top.is_none() && self.is_hidden(self.anchor.file_idx) {
            self.anchor = ScrollAnchor {
                file_idx: self.anchor.file_idx,
                row: RowKey::Lead,
                offset_px: 0.0,
            };
        }
        self.relayout_slots(top);
    }

    /// The top of the document: the lead of the first shown file (of the
    /// first file when every file is hidden).
    pub fn top_anchor(&self) -> ScrollAnchor {
        ScrollAnchor {
            file_idx: self
                .order
                .get(self.top_slot() as usize)
                .copied()
                .unwrap_or(0),
            row: RowKey::Lead,
            offset_px: 0.0,
        }
    }

    /// The slot whose lead holds the prelude: the first shown one, else the
    /// first.
    pub(crate) fn top_slot(&self) -> u32 {
        self.shown.map_or(0, |(first, _)| first)
    }

    /// The slot holding the gap below the last card: the last shown one.
    pub(crate) fn last_shown_slot(&self) -> Option<u32> {
        self.shown.map(|(_, last)| last)
    }

    /// The next shown file after file `file_idx` in display order.
    pub(crate) fn next_shown(&self, file_idx: u32) -> Option<u32> {
        let start = self.slot(file_idx) + 1;
        self.shown_files(SlotRange {
            start,
            end: self.len(),
        })
        .next()
    }

    /// The previous shown file before file `file_idx` in display order.
    pub(crate) fn prev_shown(&self, file_idx: u32) -> Option<u32> {
        let mut offset = self.heights.prefix(self.slot(file_idx) as usize);
        loop {
            let slot = self.heights.last_before(offset)?;
            let f = self.order[slot];
            if !self.is_hidden(f) {
                return Some(f);
            }
            offset = self.heights.prefix(slot);
        }
    }

    /// The anchor's offset into the top of the document, the first shown
    /// file's lead (the prelude and the gap below it), when it is there.
    fn offset_in_top_lead(&self) -> Option<f32> {
        let top = self.top_anchor();
        (self.anchor.file_idx == top.file_idx && self.anchor.row == RowKey::Lead)
            .then_some(self.anchor.offset_px)
    }

    /// After the order or the hidden set changed: finds the first and last
    /// shown slots, rebuilds the height index (leads and the last card's gap
    /// move with them) and keeps the anchor, or its place in the top lead
    /// (`top`, from [`Document::offset_in_top_lead`]).
    fn relayout_slots(&mut self, top: Option<f32>) {
        let shown = |f: &u32| !self.entries[*f as usize].hidden;
        let first = self.order.iter().position(shown);
        let last = self.order.iter().rposition(shown);
        self.shown = first.zip(last).map(|(a, b)| (a as u32, b as u32));
        self.reindex();
        if let Some(offset_px) = top {
            self.anchor = ScrollAnchor {
                offset_px,
                ..self.top_anchor()
            };
        }
        self.rebase();
    }
}
