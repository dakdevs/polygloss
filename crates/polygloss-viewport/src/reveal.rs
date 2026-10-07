//! Viewport reveals (design §11.16, ADR-0030 M3–M8 and "Viewport reveals"):
//! a body that opens or closes by pointer glides instead of jumping.
//!
//! The model commits first (the collapse flags, a section's open state, the
//! revealed context, the heights, the scroll anchor and hit-testing take
//! their end state at once); a reveal only displaces paint. Each body of a
//! reveal changes from `h0` to `h1` on the commit frame, all of them on one
//! track:
//!
//! | Body          | What changes                                     | Its content as painted                                   |
//! | ------------- | ------------------------------------------------ | -------------------------------------------------------- |
//! | Card (M3, M5) | a file card below its header: its rows and its bottom padding, which a collapsed card does not have | its rows at the screen y they had before the commit, cut by a curtain the card's bottom padding above the frame |
//! | Section (M4)  | the region below a section's band: its cards     | opening, the cards at their place, cut the same way (a card the frame's edge crosses closes there); closing, copies of the cards that were on screen ([`Ghost`]), inert |
//! | Rows (M6, M7) | a run of a file's rows: a gap's inserted rows, a host block's row | at their place (↑20's ride with what follows them), cut at the frame's edge |
//! | Prelude (M8)  | the host's prelude                               | cut at the frame's edge                                  |
//!
//! `h1` is read from the live document on every frame not frozen, so a
//! host block measured or a wrapped row corrected after the commit (the
//! commit frame lays out and measures what the motion uncovers) moves where
//! the motion ends. With the travel clamped to the viewport, `D = min(|h1 −
//! h0|, viewport height)`, and the track's openness `v` (0 closed, 1 open,
//! eased by SLIDE), a body's painted height is `h = min(h0, h1) + D · v`:
//! `h0 + D · s` opening and `h1 + D · (1 − s)` closing. Its frame ends at
//! its top + `h`, and everything after it is painted at its committed y plus
//! the sum of `h − h1` over the bodies above it, so the next card stays
//! `CARDS` below the frame. Whatever the clamp skips lies below the viewport
//! at both ends. A chevron shares its body's track. Values are quantized to
//! device pixels once per frame.
//!
//! Mark all viewed (M5) is one compound reveal over the bodies that were on
//! screen before the commit; files off screen commit still. A section's
//! band off screen, a close that moves the anchor, a gap insert of more
//! than one viewport or [`MAX_GAP_ROWS`] rows, keyboard and programmatic
//! changes, and a card whose data is not loaded snap. A block's or the
//! prelude's reveal waits for its measured height ([`Pending`]): it starts
//! on the frame that measures it, and snaps if that frame does not.
//!
//! A pinned header's collapse moves the anchor to the header (it stays at
//! the viewport's top; its rows stay where they were under the curtain and
//! the next card rises from the bottom); a reversal puts the anchor back,
//! so the rows end where they are painted.
//! The commit frame also builds the settled frame once, so the rows the
//! motion uncovers are laid out and shaped then and later frames shape
//! nothing. Reduced: no displacement or rotation; the content fades in
//! place under a veil of its container's background (the card under rows,
//! the canvas under a section or the prelude), in over QUICK at the end
//! layout, out over MICRO in the held frame, then the height snaps.
//!
//! The viewport settles a running reveal before any change to its layout,
//! scroll or size ([`DiffViewport::after_scroll`], a resize,
//! [`DiffViewport::set_blocks`], [`DiffViewport::invalidate_block`],
//! [`DiffViewport::set_prelude`]); the host freezes and settles it for key
//! downs and mouse downs ([`DiffViewport::freeze_motion`],
//! [`DiffViewport::settle_motion`]). A closing card or section is closed
//! in the model from the commit, so its rows take no clicks, and an
//! occluding hitbox over its band takes the pointer from the host blocks
//! still painted there (rule 9). One reveal runs at a time.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{App, Context, Window};

use crate::document::{BlockId, ScrollAnchor};
use crate::motion::{
    self, Initiator, Motion, MotionPolicy, Play, Reduced, ReducedPlay, Settle, Track, quantize,
    tokens,
};
use crate::paint_rows::painted_header_y;
use crate::view::DiffViewport;

/// A disclosure: a file card (M3), a section (M4), Mark all viewed (M5) and
/// the prelude (M8). The pointer animates it; Reduced fades both ways. Its
/// durations follow the distance ([`disclosure`]).
pub(crate) const DISCLOSURE: Motion = Motion {
    enter: Duration::ZERO,
    exit: Duration::ZERO,
    easing: tokens::slide,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Fade,
    },
};

/// A gap's inserted rows (M6): QUICK for ↑20 and ↓20, Expand all by its
/// distance; gaps never close. Reduced fades the rows in.
pub(crate) const GAP: Motion = Motion {
    enter: tokens::QUICK,
    exit: Duration::ZERO,
    easing: tokens::slide,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Snap,
    },
};

/// A host block's height changed by a user action (M7): by its distance.
/// Reduced fades the content in at its final height; a shrink (the swap to
/// the chip) and its height snap.
pub(crate) const BLOCK: Motion = Motion {
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Snap,
    },
    ..DISCLOSURE
};

/// Rows a gap reveal inserts at most; more snap (ADR-0030 M6).
pub(crate) const MAX_GAP_ROWS: u32 = 200;

/// `base` with ADR-0030's distance durations: `reveal(travel)` in,
/// `exit(reveal(travel))` out.
pub(crate) fn disclosure(base: Motion, travel: f32, viewport_h: f32) -> Motion {
    let enter = tokens::reveal(travel, viewport_h);
    Motion {
        enter,
        exit: tokens::exit(enter),
        ..base
    }
}

/// What a body of a reveal is.
#[derive(Clone, Debug)]
pub(crate) enum BodyKind {
    /// A file card below its header.
    Card,
    /// The region below section `section`'s band, its files in slots
    /// `slot..=last_slot`. A closing one paints `ghosts`.
    Section {
        section: usize,
        last_slot: u32,
        ghosts: Rc<[Ghost]>,
    },
    /// Rows `r0..r1` of the slot's file (a gap's inserted rows, a host
    /// block's row); `from_bottom` rows ride with what follows them (↑20).
    Rows { r0: u32, r1: u32, from_bottom: bool },
    /// The host's prelude.
    Prelude,
}

/// A card of a closing section as it was painted before the commit: its
/// file and its header's and its card's bottom screen y (the files are
/// hidden in the model, so their place is kept here).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ghost {
    pub file: u32,
    pub top: f32,
    pub bottom: f32,
}

/// A body a reveal opens or closes.
#[derive(Clone, Debug)]
pub(crate) struct RevealBody {
    pub kind: BodyKind,
    /// The file's display slot (a section's first file's; the prelude's
    /// is the top slot).
    pub slot: u32,
    /// Its height as painted before the commit and after it. `h1` follows
    /// the live document on every frame not frozen.
    pub h0: f32,
    pub h1: f32,
    /// A card's: the screen y of its first row on the frame before the
    /// commit, relative to the bodies above it (its committed y for a body
    /// that was not painted): its rows stay there.
    pub rows_screen_y: f32,
}

impl RevealBody {
    /// `D = min(|h1 − h0|, viewport height)`.
    pub(crate) fn travel(&self, viewport_h: f32) -> f32 {
        (self.h1 - self.h0).abs().min(viewport_h)
    }

    /// The painted height at openness `v`: `min(h0, h1) + D · v`.
    fn height(&self, v: f32, viewport_h: f32) -> f32 {
        self.h0.min(self.h1) + self.travel(viewport_h) * v
    }
}

/// A reveal waiting for a host element's measured height (a block's, M7,
/// or the prelude's, M8): its track starts on the frame that measures it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pending {
    pub motion: Motion,
    pub initiator: Initiator,
    /// `scroll_top` when it was asked for: a measurement that moves it (an
    /// anchor above the change) snaps.
    pub scroll_top: f64,
    /// The block; `None` for the prelude.
    pub block: Option<BlockId>,
}

/// The running reveal.
pub(crate) struct Reveal {
    pub bodies: Vec<RevealBody>,
    /// The openness, 0 closed … 1 open (1 means entered).
    pub track: Track,
    /// The anchor a pinned header's collapse replaced, put back when the
    /// click is reversed.
    pub restore: Option<ScrollAnchor>,
    /// The commit frame built the settled frame (layout and shaping).
    pub primed: bool,
    /// When the last frame sampled it: a mouse down holds that frame.
    pub painted_at: Option<Instant>,
    pub pending: Option<Pending>,
}

/// What a placed body is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GeomKind {
    Card,
    Section { section: usize, last_slot: u32 },
    Rows { r0: u32, r1: u32, from_bottom: bool },
    Prelude,
}

/// What one frame paints of a body of the running reveal, in screen y
/// relative to the viewport.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RevealGeom {
    pub kind: GeomKind,
    /// A card's or rows' file; a section's first file; the top file for the
    /// prelude.
    pub file: u32,
    pub slot: u32,
    /// Where the body starts: a card's below its header (pinned or in
    /// place), a section's at its band's bottom.
    pub top: f32,
    /// The frame's edge: its top + `h`.
    pub frame_bottom: f32,
    /// Where its content is cut.
    pub curtain: f32,
    /// How far the bodies above it displace it.
    pub before: f32,
    /// How far everything after it is painted from its committed y.
    pub shift: f32,
    /// A card's first row.
    pub rows_y: f32,
    /// The content's opacity: 1, or a Reduced fade's.
    pub opacity: f32,
    /// The chevron, in degrees (0 open, −90 closed).
    pub angle: f32,
    /// Closed in the model: a closing card or section, inert.
    pub closing: bool,
    #[cfg(feature = "debug-inspect")]
    pub frozen: bool,
}

impl RevealGeom {
    /// Whether slot `slot` comes after this body, so it rides by
    /// [`RevealGeom::shift`].
    fn precedes(&self, slot: u32) -> bool {
        match self.kind {
            GeomKind::Section { last_slot, .. } => slot > last_slot,
            GeomKind::Prelude => true,
            GeomKind::Card | GeomKind::Rows { .. } => slot > self.slot,
        }
    }

    /// Whether slot `slot` is in this section's region.
    pub(crate) fn holds_slot(&self, slot: u32) -> bool {
        matches!(self.kind, GeomKind::Section { last_slot, .. }
            if (self.slot..=last_slot).contains(&slot))
    }

    /// The band its content shows in, `(top, bottom)`: from its top to its
    /// curtain.
    pub(crate) fn band(&self) -> (f32, f32) {
        (self.top, self.curtain)
    }
}

/// How far slot `slot` is painted from its committed y: by the last body
/// before it.
pub(crate) fn shift_at(geoms: &[RevealGeom], slot: u32) -> f32 {
    geoms
        .iter()
        .rev()
        .find(|g| g.precedes(slot))
        .map_or(0.0, |g| g.shift)
}

impl DiffViewport {
    /// Collapses or expands file `file_idx` as `initiator` asks. The painted
    /// chevron passes `Pointer`, which glides (ADR-0030 M3); `Keyboard`
    /// (`z`) and `Programmatic` snap, as every other path does
    /// ([`DiffViewport::set_collapsed`], [`DiffViewport::toggle_collapsed`]).
    /// A second click on the same chevron reverses from the painted height;
    /// any other toggle settles a running reveal first.
    pub fn toggle_collapsed_by(
        &mut self,
        file_idx: u32,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if file_idx >= self.doc.len() {
            return;
        }
        let collapse = !self.doc.is_collapsed(file_idx);
        let policy = motion::policy(cx);
        let reversing = self.reveal.take().filter(|r| match &r.bodies[..] {
            [b] => {
                r.pending.is_none()
                    && matches!(b.kind, BodyKind::Card)
                    && self.doc.file_at(b.slot) == file_idx
            }
            _ => false,
        });
        let animates = motion::play(&DISCLOSURE, !collapse, initiator, policy) != Play::Snap
            && if collapse {
                self.doc.file_layout(file_idx).is_some()
            } else {
                self.ensure_layout(file_idx)
            };
        if !animates {
            self.set_collapsed(file_idx, collapse, cx);
            return;
        }
        let scale = window.scale_factor().max(1.0);
        let scroll0 = self.snapped_scroll(scale);
        let h0 = self.revealed_height(file_idx, scroll0);
        let rows_before = (self.doc.body_top(file_idx) - scroll0) as f32;
        // Collapsing a pinned header moves the anchor in its body to the
        // header (`Document::set_collapsed`); a reversal puts it back.
        let pinned = self.doc.header_top(file_idx) < scroll0;
        let anchor = *self.doc.anchor();
        self.doc.set_collapsed(file_idx, collapse);
        let restore = match (collapse, &reversing) {
            (true, _) => pinned.then_some(anchor),
            (false, Some(r)) => {
                if let Some(back) = r.restore {
                    self.doc.scroll_to_anchor(back);
                }
                None
            }
            (false, None) => None,
        };
        let scroll1 = self.snapped_scroll(scale);
        let h1 = self.revealed_height(file_idx, scroll1);
        let rows_screen_y = match &reversing {
            Some(r) => r.bodies[0].rows_screen_y,
            None if collapse => rows_before,
            None => (self.doc.body_top(file_idx) - scroll1) as f32,
        };
        // Reports the top file and settles anything else that ran.
        self.after_scroll(cx);
        let body = RevealBody {
            kind: BodyKind::Card,
            slot: self.doc.slot(file_idx),
            h0,
            h1,
            rows_screen_y,
        };
        let vh = self.doc.viewport_height();
        let motion = disclosure(DISCLOSURE, body.travel(vh), vh);
        let track = reversing.map(|r| r.track);
        self.start_reveal(vec![body], motion, !collapse, initiator, track, restore, cx);
    }

    /// Collapses (or expands) `files` as `initiator` asks: one compound
    /// reveal over the cards on screen before the commit (ADR-0030 M5,
    /// the band's Mark all viewed), each slot below shifted by the changes
    /// above it, on one clock of `reveal(Σ visible Δ)`; files off screen
    /// commit still. `Pointer` glides; `Keyboard` and `Programmatic` snap,
    /// and so does the whole change when a card on screen cannot move (its
    /// data not loaded, its header pinned) or the commit moves what is on
    /// screen (the anchor moves, or the document's end clamps the scroll
    /// position).
    pub fn set_collapsed_many_by(
        &mut self,
        files: &[u32],
        collapsed: bool,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed: Vec<u32> = files
            .iter()
            .copied()
            .filter(|&f| f < self.doc.len() && self.doc.is_collapsed(f) != collapsed)
            .collect();
        changed.sort_unstable_by_key(|&f| self.doc.slot(f));
        changed.dedup();
        if changed.is_empty() {
            return;
        }
        let policy = motion::policy(cx);
        let scale = window.scale_factor().max(1.0);
        let scroll0 = self.snapped_scroll(scale);
        let vh = self.doc.viewport_height();
        let mut animates = motion::play(&DISCLOSURE, !collapsed, initiator, policy) != Play::Snap;
        // The cards on screen before the commit, top to bottom.
        let mut strip = Vec::new();
        for &f in &changed {
            if !animates {
                break;
            }
            if self.doc.is_hidden(f) {
                continue;
            }
            let top = self.doc.header_top(f) - scroll0;
            let bottom = self.doc.card_bottom(f) - scroll0;
            if top >= f64::from(vh) || bottom <= 0.0 {
                continue;
            }
            let laid = if collapsed {
                self.doc.file_layout(f).is_some()
            } else {
                self.ensure_layout(f)
            };
            if top < 0.0 || !laid {
                animates = false;
                break;
            }
            strip.push((f, self.revealed_height(f, scroll0)));
        }
        // Files above the viewport change `scroll_top`, not what is on
        // screen (the anchor keeps it still), unless the anchor moves or the
        // document's end clamps it: then the first card on screen moves.
        let first =
            |doc: &crate::document::Document, f: u32, scroll: f64| doc.header_top(f) - scroll;
        let before = strip.first().map(|&(f, _)| first(&self.doc, f, scroll0));
        for &f in &changed {
            self.doc.set_collapsed(f, collapsed);
        }
        let scroll1 = self.snapped_scroll(scale);
        let after = strip.first().map(|&(f, _)| first(&self.doc, f, scroll1));
        let moved = before != after;
        self.after_scroll(cx);
        if !animates || moved || strip.is_empty() {
            return;
        }
        let bodies: Vec<RevealBody> = strip
            .into_iter()
            .map(|(f, h0)| RevealBody {
                kind: BodyKind::Card,
                slot: self.doc.slot(f),
                h0,
                h1: self.revealed_height(f, scroll1),
                rows_screen_y: (self.doc.body_top(f) - scroll1) as f32,
            })
            .collect();
        let travel: f32 = bodies.iter().map(|b| b.travel(vh)).sum();
        let motion = disclosure(DISCLOSURE, travel, vh);
        self.start_reveal(bodies, motion, !collapsed, initiator, None, None, cx);
    }

    /// Starts a reveal of `bodies` toward open (`open`) or closed on
    /// `motion`, continuing `track` (a reversal's) or on a new one; nothing
    /// when no body changes height or the play snaps.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_reveal(
        &mut self,
        bodies: Vec<RevealBody>,
        motion: Motion,
        open: bool,
        initiator: Initiator,
        track: Option<Track>,
        restore: Option<ScrollAnchor>,
        cx: &mut Context<Self>,
    ) {
        if bodies.iter().all(|b| b.h0 == b.h1) {
            return;
        }
        let to = if open { 1.0 } else { 0.0 };
        let mut track = track.unwrap_or_else(|| Track::new(1.0 - to, 1.0));
        let now = cx.background_executor().now();
        track.retarget(to, &motion, initiator, motion::policy(cx), now);
        if track.is_settled() {
            return;
        }
        self.reveal = Some(Reveal {
            bodies,
            track,
            restore,
            primed: false,
            painted_at: None,
            pending: None,
        });
        cx.notify();
    }

    /// Holds the running reveal's sampled frame, its paint and hitboxes,
    /// requesting no frames (a mouse down, ADR-0030 rule 4): the frame last
    /// painted, which the press was aimed at (`now` before any).
    pub fn freeze_motion(&mut self, now: Instant, cx: &mut Context<Self>) {
        if let Some(reveal) = &mut self.reveal {
            reveal.track.freeze(reveal.painted_at.unwrap_or(now));
            cx.notify();
        }
    }

    /// `Settle::All` ends the running reveal at once; `Settle::Frozen` only
    /// if it is still frozen (a reveal the click started runs on).
    pub fn settle_motion(&mut self, which: Settle, cx: &mut Context<Self>) {
        let settle = self.reveal.as_ref().is_some_and(|r| match which {
            Settle::All => true,
            Settle::Frozen => r.track.is_frozen(),
        });
        if settle {
            self.reveal = None;
            cx.notify();
        }
    }

    /// Whether a reveal is running (frozen included): the host registers
    /// it with its settling while it does.
    pub fn motion_running(&self) -> bool {
        self.reveal.is_some()
    }

    /// Whether the next display frame should draw: a reveal runs, not
    /// frozen and not waiting for a measurement.
    pub(crate) fn motion_requests_frame(&self) -> bool {
        self.reveal
            .as_ref()
            .is_some_and(|r| r.pending.is_none() && !r.track.is_frozen())
    }

    /// `scroll_top` on the device pixel grid, as the painter places rows.
    pub(crate) fn snapped_scroll(&self, scale: f32) -> f64 {
        let scale = f64::from(scale);
        (self.doc.scroll_top() * scale).round() / scale
    }

    /// File `f`'s card below its header as painted at `scroll_top`: from
    /// the header's bottom (pinned or in place) to the card's bottom.
    pub(crate) fn revealed_height(&self, f: u32, scroll_top: f64) -> f32 {
        let header = painted_header_y(&self.doc, f, scroll_top);
        let body_top = header + self.doc.metrics().header_height;
        let bottom = (self.doc.card_bottom(f) - scroll_top) as f32;
        (bottom - body_top).max(0.0)
    }

    /// Body `body`'s height in the live document at `scroll_top`; `None`
    /// when it is gone (its rows were laid out again).
    fn live_height(&self, body: &RevealBody, scroll_top: f64) -> Option<f32> {
        let f = self.doc.file_at(body.slot);
        match body.kind {
            BodyKind::Card => Some(self.revealed_height(f, scroll_top)),
            BodyKind::Section { section, .. } => {
                let (top, end) = self.doc.section_region(section);
                Some((end - top) as f32)
            }
            BodyKind::Rows { r0, r1, .. } => {
                let layout = self.doc.file_layout(f)?;
                let (r0, r1) = (r0 as usize, r1 as usize);
                (r0 < r1 && r1 <= layout.len())
                    .then(|| (layout.row_top(r1) - layout.row_top(r0)) as f32)
            }
            BodyKind::Prelude => Some(self.doc.prelude_height().unwrap_or(0.0)),
        }
    }

    /// Starts a reveal waiting for a measured height once its block (or
    /// the prelude) has been measured at its width: its distance, and so
    /// its duration, is known now. A measurement that moved the scroll
    /// position, or that left the height as it was, snaps.
    pub(crate) fn start_pending(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.reveal.as_ref().and_then(|r| r.pending) else {
            return;
        };
        let measured = match p.block {
            Some(id) => self.blocks.is_measured(id),
            None => self.prelude_to_measure().is_none(),
        };
        if !measured {
            return;
        }
        let Some(reveal) = self.reveal.take() else {
            return;
        };
        if self.doc.scroll_top() != p.scroll_top {
            return;
        }
        let mut bodies = reveal.bodies;
        let Some(h1) = self.live_height(&bodies[0], p.scroll_top) else {
            return;
        };
        bodies[0].h1 = h1;
        let vh = self.doc.viewport_height();
        let motion = disclosure(p.motion, bodies[0].travel(vh), vh);
        let open = h1 > bodies[0].h0;
        self.start_reveal(bodies, motion, open, p.initiator, None, None, cx);
    }

    /// After a frame: a reveal still waiting for its measurement starts if
    /// the frame measured it late, else it snaps.
    pub(crate) fn finish_pending(&mut self, cx: &mut Context<Self>) {
        self.start_pending(cx);
        if self.reveal.as_ref().is_some_and(|r| r.pending.is_some()) {
            self.reveal = None;
        }
    }

    /// Samples the running reveal for a frame at `scroll_top` and places
    /// its bodies into `out` (nothing once it has settled, which ends it),
    /// their end heights read from the live document.
    pub(crate) fn reveal_geometry(
        &mut self,
        scale: f32,
        scroll_top: f64,
        cx: &App,
        out: &mut Vec<RevealGeom>,
    ) {
        out.clear();
        let Some(reveal) = self.reveal.as_mut() else {
            return;
        };
        let policy = motion::policy(cx);
        let pending = reveal.pending.is_some();
        let (value, opacity, frozen, settled) = if pending {
            (0.0, 1.0, false, false)
        } else {
            let now = cx.background_executor().now();
            reveal.track.follow_policy(policy, now);
            let sample = reveal.track.sample(now);
            reveal.painted_at = Some(now);
            let frozen = reveal.track.is_frozen();
            (sample.value, sample.opacity, frozen, sample.settled)
        };
        if settled {
            self.reveal = None;
            return;
        }
        // A frozen reveal holds the frame it painted.
        let bodies = reveal.bodies.len();
        for i in (0..bodies).filter(|_| !frozen && !pending) {
            let live = self
                .reveal
                .as_ref()
                .and_then(|r| self.live_height(&r.bodies[i], scroll_top));
            match (live, self.reveal.as_mut()) {
                (Some(h1), Some(r)) => r.bodies[i].h1 = h1,
                _ => {
                    self.reveal = None;
                    return;
                }
            }
        }
        let Some(reveal) = self.reveal.as_ref() else {
            return;
        };
        let doc = &self.doc;
        let vh = doc.viewport_height();
        let header_h = doc.metrics().header_height;
        let pad = doc.metrics().card_pad_bottom;
        // A Reduced fade turns nothing: the chevron shows the model from
        // the commit (a fade's first sample is still opaque).
        let turns = policy != MotionPolicy::Reduced && opacity >= 1.0;
        let mut before = 0.0;
        for body in &reveal.bodies {
            let h = if pending {
                body.h0
            } else {
                quantize(body.height(value, vh), scale)
            };
            let file = doc.file_at(body.slot);
            let (kind, top, curtain, rows_y, closing, open) = match &body.kind {
                BodyKind::Card => {
                    let header = painted_header_y(doc, file, scroll_top) + before;
                    let top = header + header_h;
                    let curtain = (top + h - pad).max(top);
                    let closing = doc.is_collapsed(file);
                    let rows_y = body.rows_screen_y + before;
                    (GeomKind::Card, top, curtain, rows_y, closing, !closing)
                }
                BodyKind::Section {
                    section, last_slot, ..
                } => {
                    let (region, _) = doc.section_region(*section);
                    let top = (region - scroll_top) as f32 + before;
                    let curtain = (top + h - pad).max(top);
                    let open = doc.sections()[*section].open;
                    let kind = GeomKind::Section {
                        section: *section,
                        last_slot: *last_slot,
                    };
                    (kind, top, curtain, top, !open, open)
                }
                BodyKind::Rows {
                    r0,
                    r1,
                    from_bottom,
                } => {
                    let row = doc
                        .file_layout(file)
                        .map_or(0.0, |l| l.row_top(*r0 as usize));
                    let top = (doc.body_top(file) + row - scroll_top) as f32 + before;
                    let kind = GeomKind::Rows {
                        r0: *r0,
                        r1: *r1,
                        from_bottom: *from_bottom,
                    };
                    (kind, top, top + h, top, false, true)
                }
                BodyKind::Prelude => {
                    let top = -scroll_top as f32;
                    (GeomKind::Prelude, top, top + h, top, false, true)
                }
            };
            let frame_bottom = top + h;
            let shift = before + h - body.h1;
            let open = if turns {
                value
            } else if open {
                1.0
            } else {
                0.0
            };
            out.push(RevealGeom {
                kind,
                file,
                slot: body.slot,
                top,
                frame_bottom,
                curtain,
                before,
                shift,
                rows_y,
                opacity,
                angle: tokens::CHEVRON_CLOSED_DEG * (1.0 - open),
                closing,
                #[cfg(feature = "debug-inspect")]
                frozen,
            });
            before = shift;
        }
    }

    /// Marks the running reveal's commit frame as built; `true` the first
    /// time (the caller builds the settled frame then). A reveal waiting
    /// for its measurement has not committed yet.
    pub(crate) fn take_reveal_commit(&mut self) -> bool {
        match &mut self.reveal {
            Some(r) if !r.primed && r.pending.is_none() => {
                r.primed = true;
                true
            }
            _ => false,
        }
    }
}
