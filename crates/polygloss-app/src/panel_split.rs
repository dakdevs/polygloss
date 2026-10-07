//! App-owned splits (ADR-0030 Panels, M1–M2; ADR-0031 M1): the sidebar and
//! the threads panel beside the main column, one element tree in every
//! state (rule 13), so nothing beside or inside a panel changes parents
//! when it opens, moves or closes (a scroll offset, a pending click and
//! keyed state survive every frame).
//!
//! ```text
//! row(id) ┬ frame: f wide, clipped, the panel's opaque fill ─┬ main
//!         │  the content laid out at W, offset f − W̄        │
//!         │  the 1 pt divider on the frame's inner edge     │
//!         └ the divider's DIVIDER_DRAG band, at rest only ──┘
//! ```
//!
//! W is the stored width ([`PanelState::width`], clamped to the caller's
//! range as the panel is drawn) and W̄ = W + `BORDER` (the panel and its
//! divider). Each frame `f = quantize(p · W̄)`, `p` the panel's [`Track`]
//! (0 hidden, 1 shown), sampled once per frame by the panel's owner
//! ([`PanelState::sample`]); the frame, the content's offset, the divider
//! and the main column's edge all derive from that one `f` (rule 11). The
//! content renders only while `f > 0` (a closed panel's content unmounts,
//! as before, so its element state resets for the next open). A closing
//! panel is covered by an occluding layer, its live content mounted under
//! it (rule 9); its divider takes no drag while it moves. A Reduced fade
//! is the content's opacity over the frame's opaque fill (rule 8), which
//! the frame paints only then.

use std::time::Instant;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, DragMoveEvent, EmptyView, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::motion::{
    self, Initiator, Motion, MotionPolicy, Play, Reduced, ReducedPlay, Sample, Settle, Track,
    quantize, tokens,
};
use crate::space::{layout, stroke};

/// The window edge a panel comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelEdge {
    /// The sidebar: the frame is the row's first child; its divider is
    /// the frame's last point.
    Leading,
    /// The threads panel: the frame is the row's last child; its divider
    /// is the frame's first point.
    Trailing,
}

/// Both panels' motion (ADR-0030 M1, M2): PANEL in, its exit out, on
/// SLIDE, by the pointer and by the keyboard (⌃⌘S and the palette,
/// OQ-58); Reduced fades the content in the held frame.
pub fn motion() -> Motion {
    Motion {
        enter: tokens::PANEL,
        exit: tokens::exit(tokens::PANEL),
        easing: tokens::slide,
        animates: &[Initiator::Pointer, Initiator::Keyboard],
        reduced: Reduced {
            enter: ReducedPlay::Fade,
            exit: ReducedPlay::Fade,
        },
    }
}

/// A panel's stored width and its motion, kept by the entity that owns
/// the panel ([`PanelOwner`]).
#[derive(Clone, Debug)]
pub struct PanelState {
    /// W, stored: what a drag of the divider set (the caller's range
    /// clamps it as the panel is drawn, so a wider window brings it back).
    pub width: f32,
    /// `p`: 0 hidden, 1 shown.
    pub track: Track,
    /// This frame's sample of `track` ([`PanelState::sample`]).
    sampled: Sample,
    /// When it was taken: a mouse down freezes the motion there, so the
    /// click lands on what was painted.
    sampled_at: Option<Instant>,
    /// `p` where the running motion started (give-way's other end).
    from: f32,
    /// How the running motion plays (only a Full slide holds the
    /// viewport's layout).
    play: Play,
}

impl PanelState {
    /// At rest, `width` wide, shown or hidden.
    pub fn new(width: f32, shown: bool) -> PanelState {
        let p = if shown { 1.0 } else { 0.0 };
        PanelState {
            width,
            track: Track::new(p, 1.0),
            sampled: Sample {
                value: p,
                opacity: 1.0,
                settled: true,
            },
            sampled_at: None,
            from: p,
            play: Play::Snap,
        }
    }

    /// Moves toward shown or hidden from where it was last painted, as
    /// `initiator` plays [`motion`] under the current policy. A snap is on
    /// the screen from the next frame.
    pub fn retarget(&mut self, shown: bool, initiator: Initiator, cx: &App) {
        let to = if shown { 1.0 } else { 0.0 };
        let policy = motion::policy(cx);
        let now = cx.background_executor().now();
        self.from = self.sampled.value;
        self.play = motion::play(&motion(), shown, initiator, policy);
        self.track.retarget(to, &motion(), initiator, policy, now);
        if self.track.is_settled() {
            self.sampled = Sample {
                value: to,
                opacity: 1.0,
                settled: true,
            };
        }
    }

    /// Samples the track for this frame (once per frame, before anything
    /// reads [`PanelState::sampled`]), requesting the next frame while it
    /// runs.
    pub fn sample(&mut self, window: &mut Window, cx: &App) -> Sample {
        self.sampled = motion::sample(&mut self.track, window, cx);
        self.sampled_at = Some(cx.background_executor().now());
        self.sampled
    }

    /// This frame's sample.
    pub fn sampled(&self) -> Sample {
        self.sampled
    }

    /// Whether the panel's frame moves or its content fades this frame.
    pub fn moving(&self) -> bool {
        !self.sampled.settled
    }

    /// Whether a Full slide runs: what the viewport's layout is held for.
    pub fn sliding(&self) -> bool {
        self.moving() && self.play == Play::Animate && self.sampled.opacity == 1.0
    }

    /// W within `range`.
    pub fn clamped_width(&self, range: (f32, f32)) -> f32 {
        self.width.clamp(range.0, range.1.max(range.0))
    }

    /// The frame's width at `p` (rule 11: quantized once, at `scale`).
    fn frame_at(&self, p: f32, range: (f32, f32), scale: f32) -> f32 {
        quantize(p * (self.clamped_width(range) + stroke::BORDER), scale)
    }

    /// This frame's geometry, W clamped to `range`.
    pub fn frame(&self, range: (f32, f32), scale: f32) -> PanelFrame {
        PanelFrame {
            width: self.clamped_width(range),
            f: self.frame_at(self.sampled.value, range, scale),
            opacity: self.sampled.opacity,
            moving: self.moving(),
        }
    }

    /// The frame's width at rest, shown or hidden.
    pub fn rest_frame(&self, shown: bool, range: (f32, f32), scale: f32) -> f32 {
        self.frame_at(if shown { 1.0 } else { 0.0 }, range, scale)
    }

    /// While the panel moves, the frame's widths at the motion's two ends:
    /// where it started and where it goes (`shown`).
    pub fn ends(&self, shown: bool, range: (f32, f32), scale: f32) -> Option<(f32, f32)> {
        self.moving().then(|| {
            (
                self.frame_at(self.from, range, scale),
                self.rest_frame(shown, range, scale),
            )
        })
    }
}

/// One frame of a panel, from [`PanelState::frame`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelFrame {
    /// W, clamped: the content's width.
    pub width: f32,
    /// The frame's width, the divider included.
    pub f: f32,
    /// The content's opacity (a Reduced fade; 1 otherwise).
    pub opacity: f32,
    pub moving: bool,
}

/// The entity that owns a panel: its [`PanelState`] and the model it
/// shows.
pub trait PanelOwner: 'static {
    fn panel(&self) -> &PanelState;
    fn panel_mut(&mut self) -> &mut PanelState;
    /// Whether the panel shows in the model (its motion's target): a
    /// closing panel is hidden from the commit.
    fn shown(&self) -> bool;
    /// The frame's opaque fill under the content while it fades (rule 8):
    /// the content's own.
    fn background(&self, cx: &App) -> Hsla;
}

/// Samples `owner`'s panel for this frame (call it once per frame from the
/// render that draws the panel, before anything reads the sample) and,
/// while it moves, registers it with `motion::settle`: `action` is its
/// toggle, whose own shortcut retargets it.
pub fn sample<O: PanelOwner>(
    owner: &Entity<O>,
    action: fn() -> Box<dyn Action>,
    window: &mut Window,
    cx: &mut App,
) -> Sample {
    let sample = owner.update(cx, |o, cx| o.panel_mut().sample(window, cx));
    if !sample.settled {
        let (freeze, settle) = (owner.downgrade(), owner.downgrade());
        motion::settle::register(
            Some(action()),
            move |now, cx| {
                freeze
                    .update(cx, |o, cx| {
                        let panel = o.panel_mut();
                        panel.track.freeze(panel.sampled_at.unwrap_or(now));
                        cx.notify();
                    })
                    .ok();
            },
            move |which, cx| {
                settle
                    .update(cx, |o, cx| {
                        let track = &mut o.panel_mut().track;
                        if which == Settle::All || track.is_frozen() {
                            track.settle();
                            cx.notify();
                        }
                    })
                    .ok();
            },
            window,
            cx,
        );
    }
    sample
}

/// What a drag of a divider carries: its split's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DividerDrag(&'static str);

/// A panel beside `main` in a row `id` (module docs): the panel from
/// `edge`, its content from `panel` (called only while the frame is open),
/// W clamped to `range`, its motion and width in `state`. Debug selectors,
/// while the frame is open: `<id>-pane` (the frame), `<id>-divider`,
/// `<id>-drag` (the band, at rest).
#[allow(clippy::too_many_arguments)]
pub fn panel_split(
    id: &'static str,
    edge: PanelEdge,
    state: &Entity<impl PanelOwner>,
    range: (f32, f32),
    panel: impl FnOnce(&mut Window, &mut App) -> AnyElement,
    main: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (frame, shown, background) = {
        let owner = state.read(cx);
        (
            owner.panel().frame(range, window.scale_factor()),
            owner.shown(),
            owner.background(cx),
        )
    };
    let PanelFrame {
        width,
        f,
        opacity,
        moving,
    } = frame;
    let leading = edge == PanelEdge::Leading;
    let open = f > 0.0;
    // Laid out at W and riding the divider: the frame's trailing edge
    // (sidebar) or its leading one (threads panel).
    let content = open.then(|| {
        let x = if leading {
            f - width - stroke::BORDER
        } else {
            stroke::BORDER
        };
        div()
            .absolute()
            .top_0()
            .left(px(x))
            .w(px(width))
            .h_full()
            .opacity(opacity)
            .child(panel(window, cx))
    });
    let border = cx.theme().border;
    let divider = open.then(|| {
        div()
            .debug_selector(move || format!("{id}-divider"))
            .absolute()
            .top_0()
            .left(px(if leading { f - stroke::BORDER } else { 0.0 }))
            .w(px(stroke::BORDER))
            .h_full()
            .bg(border)
    });
    // A closing panel is inert from the commit: its live content stays
    // mounted under this layer.
    let cover = (moving && !shown).then(|| div().absolute().inset_0().occlude());
    let frame = div()
        .when(open, |frame| {
            frame.debug_selector(move || format!("{id}-pane"))
        })
        .relative()
        .flex_none()
        .w(px(f))
        .h_full()
        .overflow_hidden()
        // Opaque under a fading content (its own fill covers the frame
        // otherwise), so nothing beneath shows through (rule 8).
        .when(opacity < 1.0, |frame| frame.bg(background))
        .children(content)
        .children(divider)
        .children(cover);
    let main = div().flex_1().min_w_0().h_full().child(main);
    // The drag band, centred on the divider; at rest only.
    let band = (shown && !moving).then(|| {
        let half = (layout::DIVIDER_DRAG - stroke::BORDER) / 2.0;
        let inset = px(f - stroke::BORDER - half);
        div()
            .id(SharedString::from(format!("{id}-drag")))
            .debug_selector(move || format!("{id}-drag"))
            .absolute()
            .top_0()
            .h_full()
            .w(px(layout::DIVIDER_DRAG))
            .map(|band| {
                if leading {
                    band.left(inset)
                } else {
                    band.right(inset)
                }
            })
            .occlude()
            .cursor_col_resize()
            .on_drag(DividerDrag(id), |_, _, _, cx| cx.new(|_| EmptyView))
    });
    let owner = state.clone();
    div()
        .id(id)
        .relative()
        .flex()
        .size_full()
        .map(|row| {
            if leading {
                row.child(frame).child(main)
            } else {
                row.child(main).child(frame)
            }
        })
        .children(band)
        // The divider's leading edge follows the pointer, W within the range.
        .on_drag_move(move |event: &DragMoveEvent<DividerDrag>, _, cx| {
            if *event.drag(cx) != DividerDrag(id) {
                return;
            }
            let x = event.event.position.x;
            let to = if leading {
                (x - event.bounds.left()).as_f32()
            } else {
                (event.bounds.right() - x).as_f32() - stroke::BORDER
            };
            let to = to.clamp(range.0, range.1.max(range.0));
            owner.update(cx, |o, cx| {
                if o.panel().width != to {
                    o.panel_mut().width = to;
                    cx.notify();
                }
            });
        })
        .into_any_element()
}

/// Whether a policy holds what a panel motion reshapes (ADR-0030 Panels:
/// Full holds the viewport's layout at the target; Reduced changes the
/// width once).
pub fn holds_layout(cx: &App) -> bool {
    motion::policy(cx) == MotionPolicy::Full
}
