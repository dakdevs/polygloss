//! The sidebar driver (T7.10, ADR-0030 M1): the sidebar hides, then shows,
//! as its toggle's click and the show-sidebar button do, so the scenario
//! times its slide in the full app (`sidebar_anim_draw_p95_ms`,
//! `sidebar_anim_draw_max_ms`, `sidebar_commit_ms`). The tree renders
//! uncached on every frame of it, so its cost is in those numbers.

use gpui_kit::{App, Window};

use super::Driver;
use crate::motion::Initiator;

/// Each toggle flips the sidebar: a round hides it (it starts shown), then
/// shows it.
pub const DRIVER: Driver = Driver {
    name: "sidebar",
    open: toggle,
    close: toggle,
};

/// Shows the sidebar if it is hidden, else hides it, by pointer.
fn toggle(window: &mut Window, cx: &mut App) {
    crate::chrome::chrome(cx).update(cx, |c, cx| {
        let visible = c.sidebar_visible();
        c.set_sidebar_visible_by(!visible, Initiator::Pointer, window, cx);
    });
}
