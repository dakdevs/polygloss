//! The one segmented control (ADR-0031, design §11.4): the sidebar's
//! `[Files | Reviews]` and the toolbar's split | unified toggle. A track
//! `height::MD` tall at radius `MD`, its rim and the gap between segments
//! `pad::RIM`; icon segments `height::SM` tall and `height::MD` wide at the
//! concentric radius (`SM`). The selected segment is filled; every enabled
//! one shows press ink (ADR-0030) and takes clicks. A press anywhere on the
//! control (a segment, a disabled one, the rim) never moves the window.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Toggled, Window, div, px,
};

use crate::space::{height, pad, radius, size};

/// One segment: its element id (also its debug selector), its icon, its
/// accessibility label, its tooltip, and whether it takes clicks.
pub struct SegmentSpec {
    pub id: &'static str,
    pub icon: IconName,
    pub label: SharedString,
    pub tooltip: SharedString,
    pub enabled: bool,
}

/// What a click on segment `ix` does.
type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A segmented control: build it with [`segmented`].
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: &'static str,
    segments: Vec<SegmentSpec>,
    selected: usize,
    on_select: OnSelect,
    /// A debug selector on the selected segment's icon.
    selected_marker: Option<&'static str>,
}

/// A segmented control `id` (its element id and debug selector) of
/// `segments`, `selected` filled; a click on an enabled segment calls
/// `on_select` with its index.
pub fn segmented(
    id: &'static str,
    segments: Vec<SegmentSpec>,
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> SegmentedControl {
    SegmentedControl {
        id,
        segments,
        selected,
        on_select: Rc::new(on_select),
        selected_marker: None,
    }
}

impl SegmentedControl {
    /// Marks the selected segment's icon with debug selector `selector`
    /// (tests read which segment is selected).
    pub fn selected_marker(mut self, selector: &'static str) -> Self {
        self.selected_marker = Some(selector);
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        // Concentric with the track: its radius less the rim.
        let segment_radius = radius::inner(radius::MD, pad::RIM);
        let id = self.id;
        let segments = self.segments.into_iter().enumerate().map(|(ix, spec)| {
            let selected = ix == self.selected;
            let on_select = self.on_select.clone();
            let icon = Icon::new(spec.icon).with_size(px(size::ICON));
            let tooltip = spec.tooltip.clone();
            div()
                .id(spec.id)
                .debug_selector(move || spec.id.into())
                .relative()
                .flex()
                .items_center()
                .justify_center()
                // An icon segment is as wide as its track is tall.
                .w(px(height::MD))
                .h(px(height::SM))
                .rounded(px(segment_radius))
                .text_color(if !spec.enabled {
                    theme.muted_foreground.opacity(0.4)
                } else if selected {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .when(selected, |s| s.bg(theme.tab_active).shadow_xs())
                .when(spec.enabled, |s| {
                    s.child(crate::chrome::ink_layer(segment_radius, cx))
                        .on_click(move |_, window, cx| on_select(ix, window, cx))
                })
                .child(match self.selected_marker.filter(|_| selected) {
                    Some(marker) => div()
                        .debug_selector(move || marker.into())
                        .flex()
                        .child(icon)
                        .into_any_element(),
                    None => icon.into_any_element(),
                })
                .role(Role::Button)
                .aria_label(spec.label)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .tooltip(crate::review_tab::toolbar::tooltip(tooltip))
        });
        h_flex()
            .id(id)
            .debug_selector(move || id.into())
            .flex_none()
            // A control: a press on it never moves the window.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .h(px(height::MD))
            .p(px(pad::RIM))
            .gap(px(pad::RIM))
            .rounded(px(radius::MD))
            .bg(theme.tab_bar_segmented)
            .children(segments)
    }
}
