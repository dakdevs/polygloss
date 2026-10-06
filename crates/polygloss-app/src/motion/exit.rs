//! A surface leaving after its model (ADR-0030 rule 9, Exits). GPUI frees
//! elements every frame, so an exit keeps data, never an element: a
//! snapshot and the surface's inert render function, re-rendered each frame
//! until the exit's longest channel settles.
//!
//! The copy has no listeners, focus handles, tab stops or tooltips (its
//! render function's contract); [`ExitHitbox::Occlude`] adds an occluding
//! hitbox over it. Reduced: opacity over MICRO with no offset; a height
//! channel holds 1 until the fade settles, then the exit ends and its slot
//! closes at once.

use gpui_kit::{
    AnyElement, App, Bounds, Div, InteractiveElement as _, ParentElement as _, Pixels, Point,
    Styled as _, Window, div, point, px,
};

use super::{Initiator, Motion, Play, Track, policy, quantize};

/// Where the copy is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// Absolutely placed at these bounds in its parent; takes no layout.
    Overlay(Bounds<Pixels>),
    /// In its list: a slot of the copy's known rest `height` (never
    /// measured) and the list gap above it (0 if none).
    InFlow { height: Pixels, gap: Pixels },
}

/// Whether the copy blocks the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitHitbox {
    None,
    Occlude,
}

/// How an exit plays.
pub struct ExitSpec<'a> {
    pub placement: Placement,
    pub hitbox: ExitHitbox,
    /// The opacity channel (the offset rides it): its exit duration and
    /// easing.
    pub motion: &'a Motion,
    /// Where the copy travels by the end (toward the edge it entered from).
    pub travel: Point<Pixels>,
    /// The slot's height channel, if the slot closes.
    pub height: Option<&'a Motion>,
}

/// What an exit draws on one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExitFrame {
    pub opacity: f32,
    pub offset: Point<Pixels>,
    /// The share of the slot still open, 1 → 0.
    pub height: f32,
}

/// A surface leaving: its snapshot, re-rendered every frame until it ends.
pub struct Exit<S: 'static> {
    snapshot: S,
    render: fn(&S, &mut Window, &mut App) -> AnyElement,
    placement: Placement,
    hitbox: ExitHitbox,
    travel: Point<Pixels>,
    /// 1 → 0: the copy's opacity, and the share of `travel` still to go.
    opacity: Track,
    height: Option<Track>,
}

impl<S: 'static> Exit<S> {
    /// Starts the exit at the executor clock's now; `None` when it snaps
    /// (its initiator does not animate it, or the policy is Off).
    pub fn start(
        snapshot: S,
        render: fn(&S, &mut Window, &mut App) -> AnyElement,
        spec: ExitSpec,
        initiator: Initiator,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Self> {
        let now = cx.background_executor().now();
        let policy = policy(cx);
        let mut opacity = Track::new(1.0, 1.0);
        opacity.retarget(0.0, spec.motion, initiator, policy, now);
        if opacity.is_settled() {
            return None;
        }
        // Reduced, the slot holds until the fade ends: no height channel.
        let fading = super::play(spec.motion, false, initiator, policy) == Play::Fade;
        let height = spec.height.filter(|_| !fading).map(|motion| {
            let mut height = Track::new(1.0, 1.0);
            height.retarget(0.0, motion, initiator, policy, now);
            height
        });
        Some(Exit {
            snapshot,
            render,
            placement: spec.placement,
            hitbox: spec.hitbox,
            travel: spec.travel,
            opacity,
            height,
        })
    }

    /// This frame's opacity, offset and slot share; `None` once every
    /// channel has settled (the longest decides). Call it from the owner's
    /// render: it requests frames while the exit runs.
    pub fn frame(&mut self, window: &mut Window, cx: &App) -> Option<ExitFrame> {
        let opacity = super::sample(&mut self.opacity, window, cx);
        let height = self
            .height
            .as_mut()
            .map(|height| super::sample(height, window, cx));
        if opacity.settled && height.is_none_or(|h| h.settled) {
            return None;
        }
        let gone = 1.0 - opacity.value;
        let scale = window.scale_factor();
        Some(ExitFrame {
            opacity: opacity.value * opacity.opacity,
            offset: point(
                px(quantize(self.travel.x.as_f32() * gone, scale)),
                px(quantize(self.travel.y.as_f32() * gone, scale)),
            ),
            height: height.map_or(1.0, |h| h.value),
        })
    }

    /// Freezes the exit where it is (a mouse down; the owner's
    /// registration calls it).
    pub fn freeze(&mut self, now: std::time::Instant) {
        self.opacity.freeze(now);
        if let Some(height) = &mut self.height {
            height.freeze(now);
        }
    }

    /// The copy at [`Self::frame`]'s offset inside a plain div (no element
    /// id) carrying its opacity: an overlay absolutely placed at its bounds;
    /// in flow, the slot, `height share · (height + gap)` tall with a top
    /// margin of −gap, clipped, the copy `gap` below its top (its rest y),
    /// so a share of 0 is the settled list. The caller adds its list's width
    /// styles and places the slot as the list's direct child (a negative
    /// margin inside another wrapper would not reach the list). `None` with
    /// `frame`.
    pub fn render(&mut self, window: &mut Window, cx: &mut App) -> Option<Div> {
        let frame = self.frame(window, cx)?;
        let copy = (self.render)(&self.snapshot, window, cx);
        let carrier = div().absolute().opacity(frame.opacity).child(copy);
        let carrier = match self.hitbox {
            ExitHitbox::None => carrier,
            ExitHitbox::Occlude => carrier.occlude(),
        };
        Some(match self.placement {
            Placement::Overlay(bounds) => carrier
                .left(bounds.origin.x + frame.offset.x)
                .top(bounds.origin.y + frame.offset.y)
                .w(bounds.size.width)
                .h(bounds.size.height),
            Placement::InFlow { height, gap } => {
                let slot = quantize(
                    frame.height * (height + gap).as_f32(),
                    window.scale_factor(),
                );
                // Closed, the slot leaves the flow (with the list's gap):
                // taffy sizes a list around a 0 pt item with a negative
                // margin as if the margin were 0.
                let slot_div = if slot == 0.0 { div().hidden() } else { div() };
                // No `flex_none` or `flex_shrink_0`: with the negative margin,
                // taffy then misplaces the items.
                slot_div
                    .relative()
                    .h(px(slot))
                    .mt(-gap)
                    .overflow_hidden()
                    .child(
                        carrier
                            .left(frame.offset.x)
                            .top(gap + frame.offset.y)
                            .w_full()
                            .h(height),
                    )
            }
        })
    }
}
