//! The one segmented control (ADR-0031, design §11.4): the sidebar's
//! `[Files | Reviews]` and the toolbar's split | unified toggle. A track
//! `height::MD` tall at radius `MD`, its rim and the gap between segments
//! `pad::RIM`; icon segments `height::SM` tall and `height::MD` wide at the
//! concentric radius (`SM`). Every enabled segment shows press ink
//! (ADR-0030) and takes clicks. A press anywhere on the control (a segment,
//! a disabled one, the rim) never moves the window.
//!
//! The selected segment's fill is one absolutely placed element under the
//! segments (M11, ADR-0030): a pointer's pick slides it from where it is
//! painted to the new segment over QUICK on MOVE, while the labels' colors
//! and the owner's content change on the commit frame. Its [`Track`] lives in
//! a [`SegmentedState`] the control's owner keeps (rule 13). A selection
//! that changes any other way (`s`, the palette, ⌘F, ⌘P or `⇥` forcing
//! Files, an automatic layout flip) jumps, and so does every pick under
//! Reduce Motion.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, BoxShadow, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, Toggled, Window, div, px,
};

use crate::motion::{
    self, Initiator, Motion, Reduced, ReducedPlay, Settle, Track, quantize, tokens,
};
use crate::space::{height, pad, radius, size};

/// The selected fill's motion (ADR-0030 M11): QUICK both ways on MOVE, by
/// pointer only; under Reduce Motion it jumps.
pub const FILL: Motion = Motion {
    enter: tokens::QUICK,
    exit: tokens::QUICK,
    easing: tokens::in_out,
    animates: &[Initiator::Pointer],
    reduced: Reduced {
        enter: ReducedPlay::Snap,
        exit: ReducedPlay::Snap,
    },
};

/// A segmented control's selection as its fill shows it: the fill's
/// [`Track`] (its position in segments, 0 under the first) and the segment
/// it moves to or rests under. The control's owner keeps one per control
/// (the sidebar's in `Chrome`, the layout toggle's in its review tab).
pub struct SegmentedState {
    fill: Track,
    selected: usize,
}

impl SegmentedState {
    /// At rest under segment `selected`.
    pub fn new(selected: usize) -> SegmentedState {
        SegmentedState {
            // Both directions take QUICK, so the shown value is any.
            fill: Track::new(selected as f32, 0.0),
            selected,
        }
    }

    /// Selects segment `index`, caused by `initiator`: a pointer's pick
    /// slides the fill from the position it was last painted at (a running
    /// slide retargets from there); anything else puts it under `index` at
    /// once. The control calls it before its `on_select`, whose change of the
    /// owner's model shows `index` selected from the commit frame.
    pub fn select_by(&mut self, index: usize, initiator: Initiator, cx: &mut Context<Self>) {
        if index == self.selected {
            return;
        }
        self.selected = index;
        let now = cx.background_executor().now();
        self.fill
            .retarget(index as f32, &FILL, initiator, motion::policy(cx), now);
        cx.notify();
    }

    /// The owner's model shows `selected` without [`Self::select_by`]: the
    /// fill jumps there.
    fn follow(&mut self, selected: usize) {
        if selected != self.selected {
            *self = SegmentedState::new(selected);
        }
    }
}

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
    state: Entity<SegmentedState>,
    segments: Vec<SegmentSpec>,
    selected: usize,
    on_select: OnSelect,
    /// A debug selector on the selected segment's icon.
    selected_marker: Option<&'static str>,
    /// The opacity a fade around the control paints it at ([`Self::faded`]).
    fade: f32,
}

/// A segmented control `id` (its element id and debug selector; its fill's
/// is `<id>-fill`) of `segments`, `selected` filled, its fill's motion in
/// `state`; a click on an enabled segment selects it in `state`
/// ([`SegmentedState::select_by`]), then calls `on_select` with its index.
pub fn segmented(
    id: &'static str,
    state: &Entity<SegmentedState>,
    segments: Vec<SegmentSpec>,
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> SegmentedControl {
    SegmentedControl {
        id,
        state: state.clone(),
        segments,
        selected,
        on_select: Rc::new(on_select),
        selected_marker: None,
        fade: 1.0,
    }
}

impl SegmentedControl {
    /// Marks the selected segment's icon with debug selector `selector`
    /// (tests read which segment is selected).
    pub fn selected_marker(mut self, selector: &'static str) -> Self {
        self.selected_marker = Some(selector);
        self
    }

    /// The control is inside content fading at `opacity` (a Reduced panel
    /// fade): the selected fill's shadow ramps by α³ instead of α, so it
    /// never shows through the fading fill (ADR-0030 rule 8).
    pub fn faded(mut self, opacity: f32) -> Self {
        self.fade = opacity;
        self
    }
}

/// gpui's extra-small shadow, its colors at `alpha` of their own (the
/// fade's α² under the content's own α).
fn fill_shadow(alpha: f32) -> Vec<BoxShadow> {
    let mut shadows = StyleRefinement::default()
        .shadow_xs()
        .box_shadow
        .unwrap_or_default();
    for shadow in &mut shadows {
        shadow.color = shadow.color.opacity(alpha);
    }
    shadows
}

/// The fill's position this frame, in segments: follows a selection made
/// elsewhere, samples the track, and while it slides registers it with
/// `motion::settle` (no shortcut of its own retargets it).
fn fill_position(
    state: &Entity<SegmentedState>,
    selected: usize,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    state.update(cx, |s, cx| {
        s.follow(selected);
        let sample = motion::sample(&mut s.fill, window, cx);
        if !sample.settled {
            let (freeze, settle) = (cx.weak_entity(), cx.weak_entity());
            motion::settle::register(
                None,
                move |now, cx| {
                    freeze
                        .update(cx, |s, cx| {
                            s.fill.freeze(now);
                            cx.notify();
                        })
                        .ok();
                },
                move |which, cx| {
                    settle
                        .update(cx, |s, cx| {
                            if which == Settle::All || s.fill.is_frozen() {
                                s.fill.settle();
                                cx.notify();
                            }
                        })
                        .ok();
                },
                window,
                cx,
            );
        }
        sample.value
    })
}

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let position = fill_position(&self.state, self.selected, window, cx);
        let theme = cx.theme();
        // Concentric with the track: its radius less the rim.
        let segment_radius = radius::inner(radius::MD, pad::RIM);
        let id = self.id;
        // Segments are as wide as the track is tall, `RIM` apart; the fill's
        // offset is quantized once (rule 11).
        let pitch = height::MD + pad::RIM;
        let fill = div()
            .debug_selector(move || format!("{id}-fill"))
            .absolute()
            .left(px(
                pad::RIM + quantize(position * pitch, window.scale_factor())
            ))
            .top(px(pad::RIM))
            .w(px(height::MD))
            .h(px(height::SM))
            .rounded(px(segment_radius))
            .bg(theme.tab_active)
            .shadow(fill_shadow(self.fade * self.fade));
        let segments = self.segments.into_iter().enumerate().map(|(ix, spec)| {
            let selected = ix == self.selected;
            let (state, on_select) = (self.state.clone(), self.on_select.clone());
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
                .when(spec.enabled, |s| {
                    s.child(crate::chrome::ink_layer(segment_radius, cx))
                        .on_click(move |_, window, cx| {
                            let initiator = motion::initiator(window);
                            state.update(cx, |s, cx| s.select_by(ix, initiator, cx));
                            on_select(ix, window, cx);
                        })
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
            .relative()
            .flex_none()
            // A control: a press on it never moves the window.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .h(px(height::MD))
            .p(px(pad::RIM))
            .gap(px(pad::RIM))
            .rounded(px(radius::MD))
            .bg(theme.tab_bar_segmented)
            // Under the segments: they paint over it.
            .child(fill)
            .children(segments)
    }
}
