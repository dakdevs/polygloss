//! ADR-0030's stepping protocol for motion tests (T7.2): the commit frame
//! is t = 0; advance exactly `FIRST_STEP` (16.667 ms) and draw, then advance
//! to the sampled t and draw, so t runs from the commit stamp and the
//! first-step clamp never engages (its own test drives it).

use std::time::Duration;

use gpui_kit::VisualTestContext;
use polygloss_app::motion::tokens::FIRST_STEP;

/// From the commit frame, draws the frame `since_commit` later by the
/// stepping protocol.
pub fn step_to(cx: &mut VisualTestContext, since_commit: Duration) {
    let first = since_commit.min(FIRST_STEP);
    advance(cx, first);
    if since_commit > first {
        advance(cx, since_commit - first);
    }
}

/// Advances the executor clock by `by` and draws one frame, delivering the
/// frame motion requested (its owners are notified, as a display frame
/// would).
pub fn advance(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    frame(cx);
}

/// Draws one frame at the clock's current time.
pub fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.refresh();
    });
    cx.run_until_parked();
}

/// How many frames were requested since the last frame was delivered
/// (delivering them).
pub fn requested_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}
