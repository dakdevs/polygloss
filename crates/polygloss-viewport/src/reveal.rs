//! Viewport reveals (design §11.16, ADR-0030 M3 and "Viewport reveals"): a
//! file card opening or closing by pointer glides instead of jumping.
//!
//! The model commits first ([`DiffViewport::toggle_collapsed_by`]: the
//! collapse flag, the heights, the scroll anchor and hit-testing take their
//! end state at once); the reveal only displaces paint. A body changes from
//! `h0` to `h1` on the commit frame, where `h` is the card below its header
//! as painted (the rows and the card's bottom padding, which a collapsed
//! card does not have); `h1` is read from the live document every frame,
//! so a host block measured or a wrapped row corrected after the commit
//! moves where the motion ends. With the travel clamped to the viewport, `D =
//! min(|h1 − h0|, viewport height)`, and the track's openness `v` (0
//! closed, 1 open, eased by SLIDE), the body's painted height is `h = min(h0,
//! h1) + D · v`: `h0 + D · s` opening and `h1 + D · (1 − s)` closing. The
//! card's frame ends at the body's top + `h`; the rows are cut by a curtain
//! the card's bottom padding above it (never above the body's top) and keep
//! the screen y they had before the commit; every later slot is painted at
//! its committed y + the frame's offset from its committed bottom, so the
//! next card stays `CARDS` below the frame. Whatever the clamp skips lies
//! below the viewport at both ends. The chevron shares the track. Values
//! are quantized to device pixels once per frame.
//!
//! A pinned header's collapse moves the anchor to the header (it stays at
//! the viewport's top; its rows stay where they were under the curtain and
//! the next card rises from the bottom); a reversal puts the anchor back,
//! so the rows end where they are painted.
//! The commit frame also builds the settled frame once, so the rows the
//! motion uncovers are laid out and shaped then and later frames shape
//! nothing. Reduced: no displacement or rotation; the rows fade in place
//! under a veil of the card's background (their card is opaque under them),
//! in over QUICK at the end layout, out over MICRO in the held frame, then
//! the height snaps. Keyboard and programmatic changes, and a body whose
//! data is not loaded, snap.
//!
//! The viewport settles a running reveal before any change to its layout,
//! scroll or size ([`DiffViewport::after_scroll`], a resize,
//! [`DiffViewport::set_blocks`], [`DiffViewport::invalidate_block`]); the
//! host freezes and settles it for key downs and mouse downs
//! ([`DiffViewport::freeze_motion`], [`DiffViewport::settle_motion`]). A
//! closing body is closed in the model from the commit, so its rows take no
//! clicks, and an occluding hitbox over its band takes the pointer from the
//! host blocks still painted there (rule 9). One reveal runs at a time.

use std::time::{Duration, Instant};

use gpui_kit::{App, Context, Window};

use crate::document::ScrollAnchor;
use crate::motion::{
    self, Initiator, Motion, MotionPolicy, Play, Reduced, ReducedPlay, Settle, Track, quantize,
    tokens,
};
use crate::paint_rows::painted_header_y;
use crate::view::DiffViewport;

/// The card disclosure (ADR-0030 M3): the pointer animates it, Reduced
/// fades. Its durations follow the distance ([`RevealBody::motion`]).
const CARD: Motion = Motion {
    enter: Duration::ZERO,
    exit: Duration::ZERO,
    easing: tokens::slide,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Fade,
        exit: ReducedPlay::Fade,
    },
};

/// A body a reveal opens or closes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RevealBody {
    /// The file's display slot.
    pub slot: u32,
    /// The card below its header as painted before the commit and after it.
    /// `h1` follows the live document on every frame not frozen: a host
    /// block measured or a wrapped row corrected after the commit (the
    /// commit frame lays out and measures what the motion uncovers) moves
    /// where the motion ends, so its last frame is the settled one.
    pub h0: f32,
    pub h1: f32,
    /// Screen y of the body's first row on the frame before the commit (its
    /// committed y for a body that was not painted): its rows stay there.
    pub rows_screen_y: f32,
}

impl RevealBody {
    /// The painted height at openness `v`: `min(h0, h1) + D · v`.
    fn height(&self, v: f32, viewport_h: f32) -> f32 {
        let travel = (self.h1 - self.h0).abs().min(viewport_h);
        self.h0.min(self.h1) + travel * v
    }

    /// ADR-0030 M3: `reveal(Δv)` in, `exit(reveal(Δv))` out, by the heights
    /// the commit knows (the duration does not follow a later `h1`).
    fn motion(&self, viewport_h: f32) -> Motion {
        let enter = tokens::reveal(self.h1 - self.h0, viewport_h);
        Motion {
            enter,
            exit: tokens::exit(enter),
            ..CARD
        }
    }
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
}

/// What one frame paints of the running reveal, in screen y relative to
/// the viewport.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RevealGeom {
    pub file: u32,
    pub slot: u32,
    /// The body's top: below its header, pinned or in place.
    pub body_top: f32,
    /// The card frame's bottom: the body's top + `h`.
    pub frame_bottom: f32,
    /// Where the rows are cut.
    pub curtain: f32,
    /// How far every later slot is painted from its committed y.
    pub shift: f32,
    /// The body's first row.
    pub rows_y: f32,
    /// The rows' opacity: 1, or a Reduced fade's.
    pub opacity: f32,
    /// The chevron, in degrees (0 open, −90 closed).
    pub angle: f32,
    #[cfg(feature = "debug-inspect")]
    pub frozen: bool,
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
        let reversing = self
            .reveal
            .take()
            .filter(|r| self.doc.file_at(r.bodies[0].slot) == file_idx);
        let animates = motion::play(&CARD, !collapse, initiator, policy) != Play::Snap
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
            slot: self.doc.slot(file_idx),
            h0,
            h1,
            rows_screen_y,
        };
        let motion = body.motion(self.doc.viewport_height());
        let open = if collapse { 0.0 } else { 1.0 };
        let mut track = match reversing {
            Some(r) => r.track,
            None => Track::new(1.0 - open, 1.0),
        };
        let now = cx.background_executor().now();
        track.retarget(open, &motion, initiator, policy, now);
        if h0 == h1 || track.is_settled() {
            return;
        }
        self.reveal = Some(Reveal {
            bodies: vec![body],
            track,
            restore,
            primed: false,
            painted_at: None,
        });
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

    /// Whether the next display frame should draw: a reveal runs, not frozen.
    pub(crate) fn motion_requests_frame(&self) -> bool {
        self.reveal.as_ref().is_some_and(|r| !r.track.is_frozen())
    }

    /// `scroll_top` on the device pixel grid, as the painter places rows.
    pub(crate) fn snapped_scroll(&self, scale: f32) -> f64 {
        let scale = f64::from(scale);
        (self.doc.scroll_top() * scale).round() / scale
    }

    /// File `f`'s card below its header as painted at `scroll_top`: from
    /// the header's bottom (pinned or in place) to the card's bottom.
    fn revealed_height(&self, f: u32, scroll_top: f64) -> f32 {
        let header = painted_header_y(&self.doc, f, scroll_top);
        let body_top = header + self.doc.metrics().header_height;
        let bottom = (self.doc.card_bottom(f) - scroll_top) as f32;
        (bottom - body_top).max(0.0)
    }

    /// Samples the running reveal for a frame at `scroll_top` (`None` once
    /// it has settled, which ends it) and places it, its end height read
    /// from the live document.
    pub(crate) fn reveal_geometry(
        &mut self,
        scale: f32,
        scroll_top: f64,
        cx: &App,
    ) -> Option<RevealGeom> {
        let reveal = self.reveal.as_mut()?;
        let now = cx.background_executor().now();
        let policy = motion::policy(cx);
        reveal.track.follow_policy(policy, now);
        let sample = reveal.track.sample(now);
        reveal.painted_at = Some(now);
        if sample.settled {
            self.reveal = None;
            return None;
        }
        let frozen = reveal.track.is_frozen();
        let file = self.doc.file_at(reveal.bodies[0].slot);
        // A frozen reveal holds the frame it painted.
        let live_h1 = (!frozen).then(|| self.revealed_height(file, scroll_top));
        let reveal = self.reveal.as_mut()?;
        if let Some(h1) = live_h1 {
            reveal.bodies[0].h1 = h1;
        }
        let body = reveal.bodies[0];
        let doc = &self.doc;
        let h = quantize(body.height(sample.value, doc.viewport_height()), scale);
        let header = painted_header_y(doc, file, scroll_top);
        let body_top = header + doc.metrics().header_height;
        let frame_bottom = body_top + h;
        let committed = (doc.card_bottom(file) - scroll_top) as f32;
        // A Reduced fade turns nothing: the chevron shows the model from
        // the commit (a fade's first sample is still opaque).
        let open = if policy == MotionPolicy::Reduced || sample.opacity < 1.0 {
            if doc.is_collapsed(file) { 0.0 } else { 1.0 }
        } else {
            sample.value
        };
        Some(RevealGeom {
            file,
            slot: body.slot,
            body_top,
            frame_bottom,
            curtain: (frame_bottom - doc.metrics().card_pad_bottom).max(body_top),
            shift: frame_bottom - committed,
            rows_y: body.rows_screen_y,
            opacity: sample.opacity,
            angle: tokens::CHEVRON_CLOSED_DEG * (1.0 - open),
            #[cfg(feature = "debug-inspect")]
            frozen,
        })
    }

    /// Marks the running reveal's commit frame as built; `true` the first
    /// time (the caller builds the settled frame then).
    pub(crate) fn take_reveal_commit(&mut self) -> bool {
        match &mut self.reveal {
            Some(r) if !r.primed => {
                r.primed = true;
                true
            }
            _ => false,
        }
    }
}
