//! The accordion driver (T7.13, ADR-0030 M9): the active review's Files
//! segment switches from its first shown panel to the second, then back, as
//! their headers' clicks do, so the scenario times the switch's frames in
//! the full app (`accordion_anim_draw_p95_ms`, `accordion_anim_draw_max_ms`).
//! A review with fewer than two panels (nothing categorized, as on typical)
//! draws nothing, and the scenario counts a timeout.

use gpui_kit::{App, Window};

use super::Driver;
use crate::motion::Initiator;

/// A round opens the second panel, then the first again.
pub const DRIVER: Driver = Driver {
    name: "accordion",
    open: |window, cx| switch_to(1, window, cx),
    close: |window, cx| switch_to(0, window, cx),
};

/// Opens the `nth` shown panel of the active review's tree by pointer, the
/// sidebar first shown on its Files segment.
fn switch_to(nth: usize, window: &mut Window, cx: &mut App) {
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    let Some(tab) = main.read(cx).tabs().active_item().review().cloned() else {
        return;
    };
    let Some(tree) = crate::tree::file_tree(tab.read(cx)).cloned() else {
        return;
    };
    crate::chrome::show_files(window, cx);
    tree.update(cx, |t, cx| {
        let shown: Vec<_> = (t.panels().panels().iter())
            .filter(|p| p.is_shown())
            .map(|p| p.key().clone())
            .collect();
        if shown.len() > 1 {
            t.open_panel_by(&shown[nth], Initiator::Pointer, window, cx);
        }
    });
}
