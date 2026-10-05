//! Category sections in the document (design §11.6 "Sections", §11.15).
//!
//! The files of no section come first, in git order, then each section's
//! files in the host's order. A section's band is part of the lead of its
//! first file, shown or not, so a closed section is its band alone: its
//! files are hidden ([`Document::set_hidden`]'s rules), and its first file
//! keeps the band's height in its lead. Walks over shown files step over
//! that file like any hidden one; bands are found with
//! [`Document::sections_in`].
//!
//! The anchor policy: an anchor in a file that a section hides moves to the
//! section's band, `(first file, Lead, 0)`; an anchor at the top of the
//! document stays at the top; any other anchor stays put.

use super::Document;
use super::anchor::{RowKey, ScrollAnchor};
use super::slots::SlotRange;

/// A section as the document lays it out: its files, in display order, and
/// whether they show (the view keeps its label and icon, [`crate::Section`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionFiles {
    pub id: u32,
    pub files: Vec<u32>,
    pub open: bool,
}

/// No section, in [`Document`]'s per-file section index.
pub(crate) const NO_SECTION: u32 = u32::MAX;

impl Document {
    /// Replaces the sections. Each file belongs to the first section that
    /// lists it (other mentions, and indices past the end, are ignored); a
    /// section that keeps no file, or reuses an earlier section's id, is
    /// dropped. The display
    /// order becomes the files of no section in git order, then every
    /// section's files; closed sections' files are hidden and every other
    /// file is shown. An anchor in a file hidden now moves to its section's
    /// band; an anchor at the top stays at the top; any other stays put.
    pub fn set_sections(&mut self, sections: Vec<SectionFiles>) {
        let n = self.len();
        let mut of = vec![NO_SECTION; n as usize];
        let mut list: Vec<SectionFiles> = Vec::with_capacity(sections.len());
        let mut ids = Vec::with_capacity(sections.len());
        for mut s in sections {
            if ids.contains(&s.id) {
                continue;
            }
            ids.push(s.id);
            let index = list.len() as u32;
            s.files.retain(|&f| {
                let free = of.get(f as usize) == Some(&NO_SECTION);
                if free {
                    of[f as usize] = index;
                }
                free
            });
            if !s.files.is_empty() {
                list.push(s);
            }
        }
        let top = self.offset_in_top_lead();
        for (entry, &s) in self.entries.iter_mut().zip(&of) {
            entry.hidden = s != NO_SECTION && !list[s as usize].open;
            entry.band = false;
        }
        for s in &list {
            self.entries[s.files[0] as usize].band = true;
        }
        let order: Vec<u32> = (0..n)
            .filter(|&f| of[f as usize] == NO_SECTION)
            .chain(list.iter().flat_map(|s| s.files.iter().copied()))
            .collect();
        for (slot, &f) in order.iter().enumerate() {
            self.slots[f as usize] = slot as u32;
        }
        self.order = order;
        self.sections = list;
        self.section_of = of;
        self.follow_hidden_anchor(top);
        self.relayout_slots(top);
    }

    /// Opens or closes section `section` (an index into
    /// [`Document::sections`]); returns whether it changed. Closing it moves
    /// an anchor in its files to its band, unless the anchor is at the top.
    pub fn set_section_open(&mut self, section: usize, open: bool) -> bool {
        let Some(s) = self.sections.get_mut(section) else {
            return false;
        };
        if s.open == open {
            return false;
        }
        s.open = open;
        let top = self.offset_in_top_lead();
        for &f in &self.sections[section].files {
            self.entries[f as usize].hidden = !open;
        }
        self.follow_hidden_anchor(top);
        self.relayout_slots(top);
        true
    }

    /// The sections, in display order.
    pub fn sections(&self) -> &[SectionFiles] {
        &self.sections
    }

    /// The index of the section with id `id`.
    pub fn section_by_id(&self, id: u32) -> Option<usize> {
        self.sections.iter().position(|s| s.id == id)
    }

    /// The index of the section file `file_idx` belongs to.
    pub fn file_section(&self, file_idx: u32) -> Option<usize> {
        self.section_of
            .get(file_idx as usize)
            .filter(|&&s| s != NO_SECTION)
            .map(|&s| s as usize)
    }

    /// Whether file `file_idx`'s lead holds a section's band: it is the
    /// section's first file.
    pub fn has_band(&self, file_idx: u32) -> bool {
        self.entries.get(file_idx as usize).is_some_and(|e| e.band)
    }

    /// The sections whose band is in `range` (its first file's slot), in
    /// display order. Few sections exist, so this looks at each of them.
    pub fn sections_in(&self, range: SlotRange) -> impl Iterator<Item = usize> + '_ {
        (0..self.sections.len()).filter(move |&s| range.contains(self.band_slot(s)))
    }

    /// Top of section `section`'s band: the bottom of its first file's lead,
    /// right above that file's card.
    pub fn band_top(&self, section: usize) -> f64 {
        let first = self.sections[section].files[0];
        self.header_top(first) - f64::from(self.metrics.band_height)
    }

    /// The slot of section `section`'s first file, which holds its band.
    pub(crate) fn band_slot(&self, section: usize) -> u32 {
        self.slot(self.sections[section].files[0])
    }

    /// Where an anchor in hidden file `file_idx` goes: its section's band
    /// (the top of the section's first file), else the top of where the
    /// file is.
    pub(crate) fn hidden_anchor(&self, file_idx: u32) -> ScrollAnchor {
        let file_idx = self
            .file_section(file_idx)
            .map_or(file_idx, |s| self.sections[s].files[0]);
        ScrollAnchor {
            file_idx,
            row: RowKey::Lead,
            offset_px: 0.0,
        }
    }

    /// After files were hidden: an anchor in one of them goes to
    /// [`Document::hidden_anchor`], unless it is at the top (`top`, from
    /// [`Document::offset_in_top_lead`]), which stays there.
    pub(crate) fn follow_hidden_anchor(&mut self, top: Option<f32>) {
        if top.is_none() && self.is_hidden(self.anchor.file_idx) {
            self.anchor = self.hidden_anchor(self.anchor.file_idx);
        }
    }
}
