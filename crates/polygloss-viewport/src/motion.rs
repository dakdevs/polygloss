//! The motion core (design §11.16, ADR-0030): the policy, the tokens, how a
//! motion plays for an initiator ([`play`]) and [`Track`], the one sampler
//! every Polygloss motion runs on.
//!
//! A [`Track`] is a plain value its owner keeps (rule 13: never keyed
//! element state) and samples with the executor clock's `now` (rule 5:
//! never the wall clock), so tests step it. It is commit-first: a retarget
//! changes nothing but what is painted on the way to the new value.

use std::time::{Duration, Instant};

use gpui_kit::{App, Global};

/// How motion plays in this app right now (ADR-0030, Motion policy).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionPolicy {
    /// macOS Reduce Motion off: every motion as ADR-0030 lists it.
    #[default]
    Full,
    /// macOS Reduce Motion on: no travel; entrances fade in over QUICK,
    /// closes fade out over MICRO in place, then snap.
    Reduced,
    /// Screenshot, E2E and perf harnesses: everything settles at once.
    Off,
}

/// What caused a change (ADR-0030 rule 3): each motion lists the ones that
/// animate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initiator {
    Pointer,
    Keyboard,
    /// Restore, URL and MCP `focus`, follow, auto-collapse, jumps, refresh:
    /// always a snap.
    Programmatic,
}

/// The harnesses' policy (a GPUI global; `None` follows the system). Set it
/// through the app's `motion::set_override`, which keeps kit motion settled
/// too.
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionPolicyOverride(pub Option<MotionPolicy>);

impl Global for MotionPolicyOverride {}

/// The policy in effect: the harness override if set, else Reduced while
/// `App::reduce_motion` is set, else Full. Derived on every call, never
/// stored.
pub fn policy(cx: &App) -> MotionPolicy {
    match cx.try_global::<MotionPolicyOverride>().and_then(|o| o.0) {
        Some(policy) => policy,
        None if cx.reduce_motion() => MotionPolicy::Reduced,
        None => MotionPolicy::Full,
    }
}

/// ADR-0030's tokens. Motion's travel constants are its own, not spacing
/// tokens.
pub mod tokens {
    use std::time::Duration;

    /// Standalone chevrons, notice and Home-row fades, Reduced exits.
    pub const MICRO: Duration = Duration::from_millis(100);
    /// Notices in, the segmented fill, gap reveals, Reduced entrances.
    pub const QUICK: Duration = Duration::from_millis(150);
    /// The ceiling of [`reveal`].
    pub const BASE: Duration = Duration::from_millis(200);
    /// The sidebar and the threads panel (Provisional, OQ-59).
    pub const PANEL: Duration = Duration::from_millis(240);

    /// The exit or collapse of a motion that enters over `entry`:
    /// `max(100 ms, 0.75 · entry)`, rounded half up to 10 ms.
    pub fn exit(entry: Duration) -> Duration {
        let three_quarters = entry.as_micros() * 3 / 4;
        let rounded = (three_quarters + 5_000) / 10_000 * 10_000;
        Duration::from_micros(rounded as u64).max(MICRO)
    }

    /// A vertical disclosure's opening over `delta_v` pt in a viewport
    /// `viewport_h` tall: `clamp(150 + 0.1 · min(|Δv|, viewport_h), 150,
    /// 200)` ms.
    pub fn reveal(delta_v: f32, viewport_h: f32) -> Duration {
        let travel = delta_v.abs().min(viewport_h.max(0.0));
        let micros = 150_000.0 + (100.0 * travel).round();
        Duration::from_micros(micros as u64).clamp(QUICK, BASE)
    }

    /// SLIDE, `cubic_bezier(0.25, 1, 0.5, 1)`: travel and size changes
    /// over [`SHIFT`] (Provisional, OQ-59).
    pub fn slide(t: f32) -> f32 {
        gpui_kit::base::animation::cubic_bezier(0.25, 1.0, 0.5, 1.0)(t)
    }

    /// OUT, `cubic_bezier(0.16, 1, 0.3, 1)`: opacity, rotation, scale and
    /// travel up to [`SHIFT`].
    pub fn out(t: f32) -> f32 {
        gpui_kit::base::animation::cubic_bezier(0.16, 1.0, 0.3, 1.0)(t)
    }

    /// MOVE, ease-in-out cubic: something that stays on screen and changes
    /// place or angle on its own.
    pub fn in_out(t: f32) -> f32 {
        gpui_kit::base::animation::ease_in_out_cubic(t)
    }

    /// Short travel: notices.
    pub const NUDGE: f32 = 4.0;
    /// Travel: the open flow's step.
    pub const SHIFT: f32 = 8.0;
    /// A disclosure chevron closed (0° is open, pointing down).
    pub const CHEVRON_CLOSED_DEG: f32 = -90.0;
    /// The longest first step after a commit (rule 6): one 60 Hz frame.
    pub const FIRST_STEP: Duration = Duration::from_micros(16_667);
}

/// Hover and press feedback (ADR-0030, Feedback without motion): the
/// foreground over a control at these alphas, instant in every policy and
/// color only. The viewport paints them over its controls; the app's
/// `motion::ink::PressInk` puts them on custom `div` controls.
pub mod ink {
    /// Under the pointer.
    pub const HOVER: f32 = 0.06;
    /// From the mouse down to the mouse up, while the pointer is on the
    /// control.
    pub const PRESSED: f32 = 0.12;
}

/// What a reduced variant does in one direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReducedPlay {
    Snap,
    /// In: opacity over QUICK at the end value. Out: opacity over MICRO in
    /// the held frame or slot, then a snap.
    Fade,
}

/// A motion's Reduced variant. It has no geometry field: a reduced motion
/// cannot move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reduced {
    pub enter: ReducedPlay,
    pub exit: ReducedPlay,
}

/// One of ADR-0030's motions.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    /// Entering (toward the track's shown value).
    pub enter: Duration,
    /// Anything else.
    pub exit: Duration,
    pub easing: fn(f32) -> f32,
    /// The initiators that animate it (its trigger); the others snap.
    pub animates: &'static [Initiator],
    pub reduced: Reduced,
}

/// How one change plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Play {
    Animate,
    Fade,
    Snap,
}

/// How `motion` plays a change caused by `initiator` under `policy`:
/// `entering` when it moves toward its shown value.
pub fn play(motion: &Motion, entering: bool, initiator: Initiator, policy: MotionPolicy) -> Play {
    if !motion.animates.contains(&initiator) {
        return Play::Snap;
    }
    match policy {
        MotionPolicy::Full => Play::Animate,
        MotionPolicy::Off => Play::Snap,
        MotionPolicy::Reduced => match reduced(motion.reduced, entering) {
            ReducedPlay::Fade => Play::Fade,
            ReducedPlay::Snap => Play::Snap,
        },
    }
}

fn reduced(reduced: Reduced, entering: bool) -> ReducedPlay {
    if entering {
        reduced.enter
    } else {
        reduced.exit
    }
}

/// What a [`Track`] paints on one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// The geometry value. A Reduced fade never moves it: an entrance is at
    /// its end from the commit, a close holds where it was until its fade
    /// ends.
    pub value: f32,
    /// The Reduced fade's opacity; 1 for every other play.
    pub opacity: f32,
    pub settled: bool,
}

/// Why running motion settles (ADR-0030 rule 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// A key down, a scroll, a resize or a fullscreen change.
    All,
    /// After a mouse up and the effects its click queued: only a track still
    /// frozen, so a motion the click retargeted or started runs on.
    Frozen,
}

/// The sampler of one animated value, kept by the entity that owns the
/// surface.
#[derive(Clone, Debug)]
pub struct Track {
    from: f32,
    to: f32,
    /// The value that means entered (1 for an open panel).
    shown: f32,
    /// The commit stamp; `None` when settled.
    start: Option<Instant>,
    duration: Duration,
    easing: fn(f32) -> f32,
    play: Play,
    reduced: Reduced,
    /// The share of the full distance this motion spans (gpui-base's
    /// reversal rule).
    span: f32,
    /// Where a fade's opacity starts.
    fade_from: f32,
    /// Whether the first step after the commit was taken (rule 6).
    stepped: bool,
    frozen: Option<Sample>,
}

impl Track {
    /// Settled at `value`; `shown` is the value that means entered.
    pub fn new(value: f32, shown: f32) -> Self {
        Track {
            from: value,
            to: value,
            shown,
            start: None,
            duration: Duration::ZERO,
            easing: tokens::slide,
            play: Play::Snap,
            reduced: Reduced {
                enter: ReducedPlay::Snap,
                exit: ReducedPlay::Snap,
            },
            span: 1.0,
            fade_from: 1.0,
            stepped: true,
            frozen: None,
        }
    }

    /// Moves toward `to` from the value sampled at `now`. `to == shown`
    /// enters (`motion.enter`), anything else exits (`motion.exit`).
    /// Reversing a running motion lasts the new direction's duration × the
    /// share of the old one already travelled.
    pub fn retarget(
        &mut self,
        to: f32,
        motion: &Motion,
        initiator: Initiator,
        policy: MotionPolicy,
        now: Instant,
    ) {
        let current = self.sample(now);
        let entering = to == self.shown;
        let play = play(motion, entering, initiator, policy);
        let reversing = !current.settled && to == self.from && self.from != self.to;
        let span = if reversing && self.play == Play::Animate {
            let travelled = (current.value - self.from) / (self.to - self.from);
            travelled * self.span + (1.0 - self.span)
        } else {
            1.0
        };
        let base = match play {
            Play::Animate if entering => motion.enter,
            Play::Animate => motion.exit,
            Play::Fade if entering => tokens::QUICK,
            Play::Fade => tokens::MICRO,
            Play::Snap => Duration::ZERO,
        };
        self.fade_from = match (play, current.settled) {
            (Play::Fade, true) if entering => 0.0,
            (Play::Fade, true) => 1.0,
            (Play::Fade, false) => current.opacity,
            _ => 1.0,
        };
        self.from = current.value;
        self.to = to;
        self.easing = motion.easing;
        self.play = play;
        self.reduced = motion.reduced;
        self.span = span;
        self.duration = base.mul_f32(span);
        self.frozen = None;
        self.stepped = false;
        self.start = Some(now);
        if play == Play::Snap || self.duration.is_zero() {
            self.settle();
        }
    }

    /// The value to paint at `now`. The first step after a retarget is at
    /// most [`tokens::FIRST_STEP`]; a frozen track returns its frozen sample.
    pub fn sample(&mut self, now: Instant) -> Sample {
        if let Some(frozen) = self.frozen {
            return frozen;
        }
        let Some(start) = self.start else {
            return Sample {
                value: self.to,
                opacity: 1.0,
                settled: true,
            };
        };
        let mut elapsed = now.saturating_duration_since(start);
        if !self.stepped && !elapsed.is_zero() {
            self.stepped = true;
            if elapsed > tokens::FIRST_STEP {
                elapsed = tokens::FIRST_STEP;
                self.start = Some(now - tokens::FIRST_STEP);
            }
        }
        let progress = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        if progress >= 1.0 {
            self.settle();
            return self.sample(now);
        }
        let entering = self.to == self.shown;
        let (value, opacity) = match self.play {
            Play::Fade if entering => (
                self.to,
                self.fade_from + (1.0 - self.fade_from) * tokens::out(progress),
            ),
            Play::Fade => (self.from, self.fade_from * (1.0 - tokens::out(progress))),
            _ => (
                self.from + (self.to - self.from) * (self.easing)(progress),
                1.0,
            ),
        };
        Sample {
            value,
            opacity,
            settled: false,
        }
    }

    /// A mouse down: holds the value sampled at `now` until a retarget or a
    /// settle.
    pub fn freeze(&mut self, now: Instant) {
        if !self.is_settled() && self.frozen.is_none() {
            self.frozen = Some(self.sample(now));
        }
    }

    /// Jumps to the end.
    pub fn settle(&mut self) {
        self.from = self.to;
        self.start = None;
        self.frozen = None;
        self.play = Play::Snap;
    }

    pub fn is_settled(&self) -> bool {
        self.start.is_none()
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen.is_some()
    }

    /// Follows a policy switch mid-motion: to Off it settles; to Reduced a
    /// moving track stops travelling on this frame (an entrance jumps to its
    /// end, a close holds where it is) and its opacity finishes from what was
    /// visible, or it settles if its Reduced variant snaps. A settled track,
    /// or one already fading, never restarts.
    pub fn follow_policy(&mut self, policy: MotionPolicy, now: Instant) {
        if self.is_settled() || self.is_frozen() {
            return;
        }
        match policy {
            MotionPolicy::Off => self.settle(),
            MotionPolicy::Reduced if self.play == Play::Animate => {
                let entering = self.to == self.shown;
                let current = self.sample(now);
                if current.settled {
                    return;
                }
                if reduced(self.reduced, entering) == ReducedPlay::Snap {
                    self.settle();
                    return;
                }
                let travelled = if self.to == self.from {
                    1.0
                } else {
                    (current.value - self.from) / (self.to - self.from)
                };
                let (fade_from, base) = if entering {
                    (travelled, tokens::QUICK)
                } else {
                    (1.0 - travelled, tokens::MICRO)
                };
                // A close holds the frame it reached.
                self.from = if entering { self.to } else { current.value };
                self.play = Play::Fade;
                self.fade_from = fade_from;
                self.duration = base;
                self.stepped = true;
                self.start = Some(now);
            }
            _ => {}
        }
    }
}

/// `v` rounded to the device pixel grid at `scale_factor` (rule 11).
pub fn quantize(v: f32, scale_factor: f32) -> f32 {
    (v * scale_factor).round() / scale_factor
}
