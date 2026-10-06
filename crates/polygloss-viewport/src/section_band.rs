//! Category sections in the view (design §11.6 "Sections", §11.15): the
//! host's sections, their bands and the band's controls.
//!
//! The host hands the viewport sections (a label, an icon and file indices;
//! [`DiffViewport::set_sections`]); the [`crate::Document`] lays them out
//! after the other files ([`crate::document::SectionFiles`]). A band sits on
//! the canvas above its section's first card, a `height::BAR` tall: left the
//! chevron, the icon, the label and `+X −Y`; right the changed-since-viewed
//! dot, the open-thread and agent pills ([`BandFlags`]), Show / Hide and
//! Mark all viewed (Mark all unviewed once every file is viewed). It lines up
//! with the file headers (ADR-0031 C2): its chevron's box at
//! `card::CHEVRON_X` and its label (the icon its leading box) at
//! `card::PATH_X` from the card column's inner edge, its last link ending
//! `edge::CARD_TRAILING` from the inner right edge. Its counts and
//! indicators are cached here: refreshed when counts land and when the host
//! sets file flags, never per frame.
//!
//! Who opens a section: the host ([`DiffViewport::set_sections`],
//! [`DiffViewport::set_section_open`]: no event), the band's controls and
//! explicit targets ([`DiffViewport::scroll_to`] but
//! [`crate::ScrollTarget::Restore`], [`DiffViewport::go_to_file`],
//! [`DiffViewport::set_cursor`], [`DiffViewport::reveal_line`]), which emit
//! [`ViewportEvent::SectionToggled`].

use std::rc::Rc;

use gpui_kit::{Context, SharedString};

use crate::controls::{ControlAction, ControlLayer};
use crate::document::SectionFiles;
use crate::file_flags::FileFlags;
use crate::header::{
    CHEVRON_DOWN, CHEVRON_RIGHT, SLOT_ACCENT, SLOT_ADDED, SLOT_HEADER, SLOT_MUTED, SLOT_REMOVED,
};
use crate::numbers::group_digits;
use crate::paint_rows::{HEADERS, Painter, link_width};
use crate::space::{card, edge, gap, height, radius, size};
use crate::text_cache::ShapedText;
use crate::view::{DiffViewport, ViewportEvent};

/// A category section as the host hands it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The host's id, unique among the sections.
    pub id: u32,
    /// The band's label ("12 test files").
    pub label: SharedString,
    /// The band's icon (`icons/<name>.svg`), if any.
    pub icon: Option<SharedString>,
    /// Its files, in display order.
    pub files: Vec<u32>,
    pub open: bool,
}

/// A band's review indicators, aggregated from its files' [`FileFlags`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BandFlags {
    /// Open threads over its files.
    pub open_threads: u32,
    /// Some of its files have agent threads.
    pub agent: bool,
    /// Some of its files changed since viewed.
    pub changed_since_viewed: bool,
}

/// A section's size: its files, how many of them have line counts yet, and
/// their sums.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionCounts {
    pub files: u32,
    pub counted: u32,
    pub additions: u64,
    pub deletions: u64,
}

/// What the view keeps per section (in the document's order): what its band
/// shows.
#[derive(Debug, Clone, Default)]
pub(crate) struct Band {
    pub label: SharedString,
    pub icon: Option<SharedString>,
    pub counts: SectionCounts,
    pub flags: BandFlags,
    /// "Mark all unviewed" instead of "Mark all viewed".
    pub all_viewed: bool,
}

impl DiffViewport {
    /// Replaces the category sections (design §11.15): the files of no
    /// section first, in git order, then each section's files after its band.
    /// A file belongs to the first section listing it; a section without
    /// files, or with an id already used, is dropped. Closed sections' files
    /// are hidden, every other file is shown. An anchor in a file hidden now
    /// moves to its section's band, an anchor at the top stays at the top,
    /// any other stays put; a cursor, selection or ⋯ menu in a hidden file
    /// goes. No event.
    pub fn set_sections(&mut self, sections: Vec<Section>, cx: &mut Context<Self>) {
        let files = sections
            .iter()
            .map(|s| SectionFiles {
                id: s.id,
                files: s.files.clone(),
                open: s.open,
            })
            .collect();
        self.doc.set_sections(files);
        // The document keeps the first section of each id.
        self.bands = self
            .doc
            .sections()
            .iter()
            .map(|kept| {
                let s = sections.iter().find(|s| s.id == kept.id);
                s.map_or_else(Band::default, |s| Band {
                    label: s.label.clone(),
                    icon: s.icon.clone(),
                    ..Band::default()
                })
            })
            .collect();
        self.refresh_band_counts();
        self.refresh_band_flags();
        self.forget_hidden(cx);
        self.after_scroll(cx);
    }

    /// Opens or closes section `id`. Closing it moves an anchor in its files
    /// to its band (unless the anchor is at the top) and drops a cursor,
    /// selection or ⋯ menu in them. No event.
    pub fn set_section_open(&mut self, id: u32, open: bool, cx: &mut Context<Self>) {
        let Some(s) = self.doc.section_by_id(id) else {
            return;
        };
        if self.doc.set_section_open(s, open) {
            self.forget_hidden(cx);
            self.after_scroll(cx);
        }
    }

    /// Whether section `id` is open; `None` when there is no such section.
    pub fn section_open(&self, id: u32) -> Option<bool> {
        let s = self.doc.section_by_id(id)?;
        Some(self.doc.sections()[s].open)
    }

    /// The id of the section file `file_idx` is in.
    pub fn section_of(&self, file_idx: u32) -> Option<u32> {
        let s = self.doc.file_section(file_idx)?;
        Some(self.doc.sections()[s].id)
    }

    /// Section `id`'s files and their line counts so far (all zero for an
    /// unknown id).
    pub fn section_counts(&self, id: u32) -> SectionCounts {
        self.band(id)
            .map_or_else(SectionCounts::default, |b| b.counts)
    }

    /// Section `id`'s review indicators, from its files' flags.
    pub fn band_flags(&self, id: u32) -> BandFlags {
        self.band(id).map_or_else(BandFlags::default, |b| b.flags)
    }

    /// Whether section `id`'s band offers "Mark all unviewed" (`true`) or
    /// "Mark all viewed". [`DiffViewport::set_file_flags`] sets it from the
    /// Viewed flags (every file viewed); this sets it until the next flags.
    pub fn set_band_all_viewed(&mut self, id: u32, all_viewed: bool, cx: &mut Context<Self>) {
        if let Some(s) = self.doc.section_by_id(id)
            && self.bands[s].all_viewed != all_viewed
        {
            self.bands[s].all_viewed = all_viewed;
            cx.notify();
        }
    }

    fn band(&self, id: u32) -> Option<&Band> {
        self.bands.get(self.doc.section_by_id(id)?)
    }

    /// An explicit target in file `file_idx`: opens its section when it is
    /// closed and emits [`ViewportEvent::SectionToggled`].
    pub(crate) fn open_section_of(&mut self, file_idx: u32, cx: &mut Context<Self>) {
        let Some(s) = self.doc.file_section(file_idx) else {
            return;
        };
        if self.doc.set_section_open(s, true) {
            let id = self.doc.sections()[s].id;
            cx.emit(ViewportEvent::SectionToggled { id, open: true });
            cx.notify();
        }
    }

    /// The band's Show / Hide: toggles section `id` and tells the host.
    pub(crate) fn toggle_section(&mut self, id: u32, cx: &mut Context<Self>) {
        let Some(open) = self.section_open(id).map(|open| !open) else {
            return;
        };
        self.set_section_open(id, open, cx);
        cx.emit(ViewportEvent::SectionToggled { id, open });
    }

    /// Recomputes every band's counts from the pipeline's (when counts land,
    /// or are dropped by a diff option change). Returns whether a band in
    /// view changed (counts of hidden files repaint nothing else).
    pub(crate) fn refresh_band_counts(&mut self) -> bool {
        let visible = self.doc.visible(self.doc.viewport_height());
        let mut repaint = false;
        for (i, (band, s)) in self.bands.iter_mut().zip(self.doc.sections()).enumerate() {
            let mut counts = SectionCounts {
                files: s.files.len() as u32,
                ..SectionCounts::default()
            };
            for c in s.files.iter().filter_map(|&f| self.pipeline.counts(f)) {
                counts.counted += 1;
                counts.additions += u64::from(c.additions);
                counts.deletions += u64::from(c.deletions);
            }
            if band.counts != counts {
                band.counts = counts;
                repaint |= visible.contains(self.doc.band_slot(i));
            }
        }
        repaint
    }

    /// Recomputes every band's indicators and its Mark all viewed label from
    /// the files' flags.
    pub(crate) fn refresh_band_flags(&mut self) {
        for (band, s) in self.bands.iter_mut().zip(self.doc.sections()) {
            let mut flags = BandFlags::default();
            let mut all_viewed = true;
            for f in s.files.iter().filter_map(|&f| self.flags.get(f as usize)) {
                flags.open_threads += f.open_threads;
                flags.agent |= f.agent_threads;
                flags.changed_since_viewed |= f.changed_since_viewed;
                all_viewed &= f.viewed;
            }
            band.flags = flags;
            band.all_viewed = all_viewed;
        }
    }
}

impl Painter<'_> {
    /// Paints section `s`'s band with its top at `y` (relative to the
    /// viewport), across a card's inner width, and records its controls.
    /// The links always show; as the band narrows the pills and the dot go
    /// first, then the counts, and the label is cut with `…`.
    pub(crate) fn band(&mut self, s: usize, y: f32) {
        let (doc, bands, theme) = (self.doc, self.bands, self.theme);
        let (section, band) = (&doc.sections()[s], &bands[s]);
        let h = doc.metrics().band_height;
        let (x0, width) = self.inner_x_w();
        let ty = y + (h - self.geometry.row_height) / 2.0;
        let icon_y = y + (h - size::ICON) / 2.0;
        let between = gap::CONTROLS;
        self.frame.rows += 1;

        // Right to left: Mark all viewed, Show / Hide.
        let toggle = ControlAction::SectionToggle(section.id);
        let mark = if band.all_viewed {
            "Mark all unviewed"
        } else {
            "Mark all viewed"
        };
        let mark_action = ControlAction::SectionMarkViewed(section.id);
        let end = x0 + width - edge::CARD_TRAILING;
        let mut rx = self.band_link(mark_action, mark, end, y, h);
        let show = if section.open { "Hide" } else { "Show" };
        rx = self.band_link(toggle, show, rx, y, h);

        // The chevron, the icon and the label (cut to the room left), one
        // toggle from the chevron's button to the label's end.
        let chevron = if section.open {
            CHEVRON_DOWN
        } else {
            CHEVRON_RIGHT
        };
        let chevron_x = x0 + card::CHEVRON_X;
        self.icon(chevron, chevron_x, icon_y, size::ICON, theme.muted);
        let mut x = x0 + card::PATH_X;
        if let Some(icon) = &band.icon {
            self.icon(icon.clone(), x, icon_y, size::ICON, theme.muted);
            x += size::ICON + gap::ICON_LABEL;
        }
        let label = self.fitted_ui_label(&band.label, rx - between - x);
        self.text(HEADERS, x, ty, label.clone());
        x += label.shaped.width();
        let inset = (height::SM - size::ICON) / 2.0;
        let (toggle_x, toggle_y) = (chevron_x - inset, y + (h - height::SM) / 2.0);
        let toggle_w = x + inset - toggle_x;
        self.control(
            toggle,
            ControlLayer::Header,
            toggle_x,
            toggle_y,
            toggle_w,
            height::SM,
        );

        // The counts after the label, while they fit.
        let counts = band.counts;
        let mut shown_counts = None;
        if counts.counted > 0 {
            let added = format!("+{}", group_digits(counts.additions));
            let removed = format!("−{}", group_digits(counts.deletions));
            let added = self.label(&added, SLOT_ADDED, theme.stat_added);
            let removed = self.label(&removed, SLOT_REMOVED, theme.stat_removed);
            let start = x + gap::GROUP;
            let end = start + added.shaped.width() + gap::INLINE + removed.shaped.width();
            if end + between <= rx {
                self.text(HEADERS, start, ty, added);
                self.text(HEADERS, end - removed.shaped.width(), ty, removed);
                shown_counts = Some((counts.additions, counts.deletions));
                x = end;
            }
        }

        // The review indicators, right to left, while they fit.
        let flags = band.flags;
        let as_file = FileFlags {
            viewed: false,
            changed_since_viewed: false,
            open_threads: flags.open_threads,
            agent_threads: flags.agent,
        };
        let pills: Vec<_> = as_file
            .badges()
            .iter()
            .map(|b| {
                let (slot, color) = if b.accent {
                    (SLOT_ACCENT, theme.accent)
                } else {
                    (SLOT_MUTED, theme.muted)
                };
                self.pill(&b.text, b.icon, slot, color)
            })
            .collect();
        let pill_h = height::SM;
        let pill_y = y + (h - pill_h) / 2.0;
        #[cfg(feature = "debug-inspect")]
        let mut badges = Vec::new();
        let mut fits = true;
        for p in pills.iter().rev() {
            fits &= rx - between - p.width >= x + between;
            if !fits {
                break;
            }
            rx -= between + p.width;
            self.paint_pill(p, rx, pill_y, pill_h, ty);
            #[cfg(feature = "debug-inspect")]
            badges.push(p.text.text().to_owned());
        }
        let dot = size::DOT;
        let dot_shown = flags.changed_since_viewed && fits && rx - between - dot >= x + between;
        if dot_shown {
            let rect = (rx - between - dot, y + (h - dot) / 2.0, dot, dot);
            self.rounded(HEADERS, rect, theme.accent, None, radius::capsule(dot));
        }

        #[cfg(feature = "debug-inspect")]
        {
            use crate::paint_rows::{DebugContent, DebugRow};
            let marker = if section.open { '▾' } else { '▸' };
            self.debug.push(DebugRow {
                y,
                height: h,
                styled: false,
                content: DebugContent::Band(format!("{marker} {}", band.label)),
            });
            badges.reverse();
            self.debug_bands.push(crate::debug::BandDebug {
                id: section.id,
                y,
                open: section.open,
                label: band.label.to_string(),
                counts: shown_counts,
                badges,
                dot: dot_shown,
                links: vec![show.to_owned(), mark.to_owned()],
            });
        }
        #[cfg(not(feature = "debug-inspect"))]
        let _ = (shown_counts, dot_shown);
    }

    /// `text` in the UI font and the header color, cut with `…` to `room`
    /// px when it is wider.
    fn fitted_ui_label(&mut self, text: &str, room: f32) -> Rc<ShapedText> {
        let color = self.theme.header_foreground;
        let whole = self.ui_label(text, SLOT_HEADER, color);
        let width = whole.shaped.width();
        if width <= room {
            return whole;
        }
        let chars = text.chars().count();
        // Characters are of uneven width: start from the average, then drop
        // one at a time until it fits.
        let mut keep = ((chars as f32 * room / width) as usize).min(chars);
        loop {
            let cut: String = text.chars().take(keep).chain(['…']).collect();
            let shaped = self.ui_label(&cut, SLOT_HEADER, color);
            if shaped.shaped.width() <= room || keep == 0 {
                return shaped;
            }
            keep -= 1;
        }
    }

    /// A band's text link (`text` in the UI font and the accent color, a
    /// [`Painter::link`]) whose box ends at `right`, centered in the band
    /// `y..y + h`; returns its box's left edge.
    fn band_link(&mut self, action: ControlAction, text: &str, right: f32, y: f32, h: f32) -> f32 {
        let shaped = self.ui_label(text, SLOT_ACCENT, self.theme.accent);
        let left = right - link_width(&shaped);
        self.link(action, ControlLayer::Header, shaped, left, y, h);
        left
    }
}
