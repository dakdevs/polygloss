//! The threads panel driver (T7.10, ADR-0030 M2): the active review's
//! threads panel shows, then hides, as the toolbar's threads button does,
//! so the scenario times its slide in the full app
//! (`threads_anim_draw_p95_ms`, `threads_anim_draw_max_ms`,
//! `threads_commit_ms`).

use gpui_kit::{App, Window};

use super::Driver;
use crate::motion::Initiator;

/// Each toggle flips the panel: a round shows it (it starts hidden), then
/// hides it.
pub const DRIVER: Driver = Driver {
    name: "threads",
    open: toggle,
    close: toggle,
};

/// Shows the active review's threads panel if it is hidden, else hides
/// it, by pointer.
fn toggle(window: &mut Window, cx: &mut App) {
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    let Some(tab) = main.read(cx).tabs().active_item().review().cloned() else {
        return;
    };
    tab.update(cx, |t, cx| {
        let shown = !t.threads_panel_visible();
        t.set_threads_shown_by(shown, Initiator::Pointer, window, cx);
    });
}
