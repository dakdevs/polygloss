//! The Files segment's footer (design §11.5, §11.15): "Total: +X −Y" over
//! the Changes panel's files, grouped by thousands, and "Total: …" until each
//! of them that has lines is counted (binary files and submodules have none
//! and add nothing); then one chip per category section ("6 tests · 2
//! generated"). When every file is categorized it shows the chips only.
//! With categorized files its tooltip is the categories' breakdown. The
//! filter never changes it. The
//! tree renders it again when counts land (`ViewportEvent::CountsUpdated`),
//! never per frame.
//!
//! Debug selectors: `tree-footer: <its text>` (the totals, or the chips
//! alone), `tree-chips: <chips>`.

use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use polygloss_viewport::{DiffViewport, group_digits};

use crate::categories::{self, Partition, Totals};

/// The footer's height (pt).
const HEIGHT: f32 = 38.0;

/// The footer: "Total: +X −Y" over `changes` (indices into the diff; `None`
/// when every file is categorized), then `partition`'s chips, with its
/// breakdown as the tooltip.
pub fn footer(
    changes: Option<&[u32]>,
    partition: Option<&Arc<Partition>>,
    viewport: &Entity<DiffViewport>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let chips = partition.map(|p| categories::chips(p)).unwrap_or_default();
    let totals = changes.map(|files| Totals::of(files.iter().copied(), viewport.read(cx)));
    let text = match totals {
        Some(t) if t.counted => format!("Total: {}", t.lines()),
        Some(_) => "Total: …".to_owned(),
        None => categories::chips_text(&chips),
    };
    let colors = crate::theme::viewport_theme(cx);
    // The counts in the code font, as in the rows.
    let count = |text: String, color: Hsla| {
        div()
            .font_family(theme.mono_font_family.clone())
            .font_semibold()
            .text_color(color)
            .child(text)
    };
    h_flex()
        .id("tree-footer")
        .debug_selector(move || format!("tree-footer: {text}"))
        .flex_none()
        .h(px(HEIGHT))
        .px_3()
        .gap_1()
        .border_t_1()
        .border_color(theme.border)
        .text_sm()
        .when_some(totals, |row, t| {
            let row = row.child(div().text_color(theme.muted_foreground).child("Total:"));
            if t.counted {
                row.child(count(
                    format!("+{}", group_digits(t.additions)),
                    colors.stat_added,
                ))
                .child(count(
                    format!("−{}", group_digits(t.deletions)),
                    colors.stat_removed,
                ))
            } else {
                row.child(div().text_color(theme.muted_foreground).child("…"))
            }
        })
        .when_some(
            categories::chips_element("tree-chips", &chips, cx),
            |row, chips| row.child(div().w(px(6.))).child(chips),
        )
        .when_some(
            partition.and_then(|p| categories::breakdown_tooltip(p, viewport)),
            |row, tooltip| row.tooltip(tooltip),
        )
        .into_any_element()
}
