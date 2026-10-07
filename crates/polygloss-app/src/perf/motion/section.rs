//! The section driver (T7.11, ADR-0030 M4): the first category section of
//! the active review opens, then closes, as its band's Show / Hide does,
//! with its band at the top of the viewport, so the scenario times the
//! region's reveal in the full app (`section_anim_draw_p95_ms`,
//! `section_anim_draw_max_ms`; budgeted on linux, where the corpus has
//! categorized files).

use gpui_kit::{App, Window};
use polygloss_viewport::{RowKey, ScrollAnchor};

use super::Driver;
use crate::motion::Initiator;

/// Each toggle flips the first section: a round opens it, then closes it
/// (it starts closed, as a section does unless every file is categorized).
pub const DRIVER: Driver = Driver {
    name: "section",
    open: toggle,
    close: toggle,
};

/// Brings the first section's band into view when it is not (its first
/// file's lead at the top) and toggles it by pointer. A review without
/// sections draws nothing (the scenario counts a timeout).
fn toggle(window: &mut Window, cx: &mut App) {
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    let Some(tab) = main.read(cx).tabs().active_item().review().cloned() else {
        return;
    };
    let viewport = tab.read(cx).viewport.clone();
    viewport.update(cx, |v, cx| {
        let Some(first) = v.document().sections().first().map(|s| s.files[0]) else {
            return;
        };
        let doc = v.document();
        let y = doc.band_top(0) - doc.scroll_top();
        let fits = f64::from(doc.viewport_height() - doc.metrics().band_height);
        if !(0.0..=fits).contains(&y) {
            let band = ScrollAnchor {
                file_idx: first,
                row: RowKey::Lead,
                offset_px: 0.0,
            };
            v.scroll_to_anchor(band, cx);
        }
        v.toggle_section_by(0, Initiator::Pointer, window, cx);
    });
}
