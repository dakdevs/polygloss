//! The card driver (T7.8, ADR-0030 M3): the card at the top of the active
//! review collapses, then expands, as its chevron's click does, so the
//! scenario times the reveal's frames in the full app
//! (`card_anim_draw_p95_ms`, `card_anim_draw_max_ms`).

use gpui_kit::{App, Window};

use super::Driver;
use crate::motion::Initiator;

/// Each toggle flips the top card: a round collapses it, then expands it.
pub const DRIVER: Driver = Driver {
    name: "card",
    open: toggle,
    close: toggle,
};

/// Toggles the card at the top of the active review (the scroll anchor's
/// file) by pointer.
fn toggle(window: &mut Window, cx: &mut App) {
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    let Some(tab) = main.read(cx).tabs().active_item().review().cloned() else {
        return;
    };
    let viewport = tab.read(cx).viewport.clone();
    viewport.update(cx, |v, cx| {
        let top = v.anchor().file_idx;
        v.toggle_collapsed_by(top, Initiator::Pointer, window, cx);
    });
}
