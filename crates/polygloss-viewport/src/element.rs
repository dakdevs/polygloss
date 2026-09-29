//! `DiffElement`: the GPUI element that paints a [`DiffViewport`].
//!
//! Prepaint asks the view for the frame's display list (visible rows only,
//! shaped through the cache); paint replays it layer by layer, every quad
//! before any text so GPUI batches them into few draw calls, and wires the
//! scroll wheel. Both phases are timed and reported as
//! [`crate::ViewportEvent::FrameStats`].

use std::time::{Duration, Instant};

use gpui_kit::{
    App, Bounds, ContentMask, DispatchPhase, Element, ElementId, Entity, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Pixels, ScrollWheelEvent, Style,
    Window, fill, px, relative,
};

use crate::paint_rows::Frame;
use crate::view::{DiffViewport, FrameStats};

pub(crate) struct DiffElement {
    view: Entity<DiffViewport>,
}

impl DiffElement {
    pub fn new(view: Entity<DiffViewport>) -> DiffElement {
        DiffElement { view }
    }
}

pub(crate) struct Prepainted {
    frame: Option<Frame>,
    hitbox: Hitbox,
    prepaint: Duration,
}

impl IntoElement for DiffElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for DiffElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.flex_grow = 1.0;
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepainted {
        let started = Instant::now();
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let frame = self
            .view
            .update(cx, |view, cx| view.prepare_frame(bounds, window, cx));
        Prepainted {
            frame: Some(frame),
            hitbox,
            prepaint: started.elapsed(),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepainted: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let Some(frame) = prepainted.frame.take() else {
            return;
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for layer in &frame.layers {
                let clip = layer.clip.map(|bounds| ContentMask { bounds });
                window.with_content_mask(clip, |window| {
                    for (quad, color) in &layer.quads {
                        window.paint_quad(fill(*quad, *color));
                    }
                });
            }
            for layer in &frame.layers {
                let clip = layer.clip.map(|bounds| ContentMask { bounds });
                window.with_content_mask(clip, |window| {
                    for (origin, text) in &layer.texts {
                        text.shaped.paint(*origin, frame.line_height, window, cx);
                    }
                });
            }
        });

        let view = self.view.clone();
        let hitbox = prepainted.hitbox.clone();
        let line_height = frame.line_height;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                let delta = event.delta.pixel_delta(line_height.max(px(1.)));
                view.update(cx, |view, cx| view.scroll_by(-delta.y.as_f32(), cx));
                cx.stop_propagation();
            }
        });

        let stats = FrameStats {
            prepaint: prepainted.prepaint,
            paint: started.elapsed(),
            visible_rows: frame.rows,
            shaped_lines: frame.shaped,
            loading_rows: frame.loading,
            unhighlighted_rows: frame.unhighlighted,
        };
        self.view
            .update(cx, |view, cx| view.finish_frame(frame, stats, cx));
    }
}
