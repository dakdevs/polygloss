//! Rows leaving Home's list (M14, ADR-0030). The list drops a removed row on
//! the commit frame; [`Removals`] then draws, by pointer, an inert copy of
//! it at its y while its slot (the card and the list's gap above it) closes
//! over `exit(BASE)` on SLIDE and the copy fades over MICRO on OUT, the rows
//! below following. A removal of more than [`ANIMATED_REMOVALS`] rows, or
//! one that empties its section, snaps; under Reduce Motion the copy fades
//! in its slot, which then closes at once.

use std::time::Instant;

use gpui_kit::{App, Context, SharedString, Styled as _, Window, px};

use super::HomeView;
use super::row::{self, HomeRow, RowCopy};
use crate::motion::exit::{Exit, ExitHitbox, ExitSpec, Placement};
use crate::motion::{self, Initiator, Motion, Recorded, Reduced, ReducedPlay, Settle, tokens};
use crate::space::{gap, layout};

/// The most rows one removal animates: a larger prune snaps.
pub const ANIMATED_REMOVALS: usize = 3;

/// A removed row's copy fading: MICRO on OUT, by pointer; under Reduce
/// Motion it fades in its slot.
const ROW_FADE: Motion = Motion {
    enter: tokens::MICRO,
    exit: tokens::MICRO,
    easing: tokens::out,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Snap,
        exit: ReducedPlay::Fade,
    },
};

/// A removed row's slot closing: `exit(BASE)` on SLIDE, by pointer.
fn row_close() -> Motion {
    Motion {
        enter: tokens::BASE,
        exit: tokens::exit(tokens::BASE),
        easing: tokens::slide,
        animates: &[Initiator::Pointer],
        reduced: Reduced {
            enter: ReducedPlay::Snap,
            exit: ReducedPlay::Fade,
        },
    }
}

/// A row removed from the list: where it was (the index of the row now in
/// its place, and whether it was in Awaiting you) and the row.
pub struct Removed {
    pub before: usize,
    pub awaiting: bool,
    pub row: HomeRow,
}

/// One row leaving: its copy and where its slot sits.
struct Leaving {
    before: usize,
    awaiting: bool,
    review_id: String,
    exit: Exit<RowCopy>,
}

/// A leaving row's slot this frame: the index of the row it sits before,
/// whether it was in Awaiting you (its slot comes before Recent's title),
/// and the slot (the list's direct child, without the rows' width).
pub struct Slot {
    pub before: usize,
    pub awaiting: bool,
    pub element: gpui_kit::Div,
}

/// The rows leaving Home's list, kept by its `HomeView` (rule 13).
#[derive(Default)]
pub struct Removals {
    leaving: Vec<Leaving>,
    /// Whether the last mouse down froze them.
    frozen: bool,
}

impl Removals {
    /// Ends every removal under way (any other change of the list).
    pub fn clear(&mut self) {
        self.leaving.clear();
    }

    /// Starts `removed`'s exits as `initiator` plays them, their relative
    /// times read at `now_ms`; it snaps (starts nothing) for more than
    /// [`ANIMATED_REMOVALS`] rows or when `emptied` a section.
    pub fn start(
        &mut self,
        removed: Vec<Removed>,
        emptied: bool,
        now_ms: i64,
        initiator: Initiator,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.clear();
        if emptied || removed.len() > ANIMATED_REMOVALS {
            return;
        }
        let utc_offset_s = row::local_utc_offset_s(now_ms);
        let close = row_close();
        for Removed {
            before,
            awaiting,
            row,
        } in removed
        {
            let spec = ExitSpec {
                placement: Placement::InFlow {
                    height: px(layout::HOME_CARD),
                    gap: px(gap::CARDS),
                },
                hitbox: ExitHitbox::Occlude,
                motion: &ROW_FADE,
                travel: Default::default(),
                height: Some(&close),
            };
            let review_id = row.review_id().to_owned();
            let copy = RowCopy {
                row,
                now_ms,
                utc_offset_s,
            };
            if let Some(exit) = Exit::start(copy, row::render_copy, spec, initiator, window, cx) {
                self.leaving.push(Leaving {
                    before,
                    awaiting,
                    review_id,
                    exit,
                });
            }
        }
        self.frozen = false;
    }

    /// This frame's slots, in list order; finished exits go. Call it from
    /// Home's render.
    pub fn slots(&mut self, window: &mut Window, cx: &mut App) -> Vec<Slot> {
        let mut slots = Vec::with_capacity(self.leaving.len());
        self.leaving.retain_mut(|leaving| {
            let Some((frame, element)) = leaving.exit.draw(window, cx) else {
                return false;
            };
            let id = SharedString::from(format!("home-row-copy-{}", leaving.review_id));
            motion::report(
                || Recorded {
                    id: id.into(),
                    offset: frame.offset,
                    opacity: frame.opacity,
                    rotation: 0.0,
                },
                cx,
            );
            slots.push(Slot {
                before: leaving.before,
                awaiting: leaving.awaiting,
                element: element.w_full().max_w(px(layout::HOME_MAX_W)),
            });
            true
        });
        slots
    }

    /// While rows leave, registers them with `motion::settle`: a mouse down
    /// freezes them, a settling input (or the mouse up after a freeze) ends
    /// them.
    pub fn register(&self, window: &mut Window, cx: &mut Context<HomeView>) {
        if self.leaving.is_empty() {
            return;
        }
        let (freeze, settle) = (cx.weak_entity(), cx.weak_entity());
        motion::settle::register(
            None,
            move |now: Instant, cx| {
                freeze
                    .update(cx, |h, cx| {
                        h.removals.freeze(now);
                        cx.notify();
                    })
                    .ok();
            },
            move |which, cx| {
                settle
                    .update(cx, |h, cx| {
                        if which == Settle::All || h.removals.frozen {
                            h.removals.clear();
                            cx.notify();
                        }
                    })
                    .ok();
            },
            window,
            cx,
        );
    }

    fn freeze(&mut self, now: Instant) {
        for leaving in &mut self.leaving {
            leaving.exit.freeze(now);
        }
        self.frozen = true;
    }
}
