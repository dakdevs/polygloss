//! `DiffElement`: the GPUI element that paints a [`DiffViewport`].
//!
//! Prepaint asks the view for the frame's display list (visible rows only,
//! shaped through the cache), renders and measures the visible host blocks
//! ([`crate::blocks`]) and inserts the hitboxes of its controls, clipped to
//! the viewport like everything it paints; paint replays it layer by layer,
//! every row quad before any row text so GPUI batches them into few draw
//! calls, then the host blocks, then the file headers on top, and wires the
//! scroll wheel and the controls. Both phases are timed and reported as
//! [`crate::ViewportEvent::FrameStats`].

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, BorderStyle, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    Pixels, ScrollWheelEvent, Style, Window, fill, px, quad, relative,
};

use crate::blocks::{self, PreparedBlock};
use crate::controls::{self, ControlLayer, Target};
use crate::paint_rows::{Frame, HEADERS, Layer};
use crate::selection;
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
    /// Visible host blocks' elements, prepainted.
    blocks: Vec<PreparedBlock>,
    hitbox: Hitbox,
    targets: Rc<[Target]>,
    /// The code cells' gutter and code hitboxes, with their pointer cursors.
    cells: Rc<[(Hitbox, CursorStyle)]>,
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
        // Hitboxes go in bottom to top: the viewport's, the blocks' (above
        // the rows), then the controls' and header strips' (a pinned header
        // covers the blocks scrolling under it, and takes their clicks).
        let (frame, blocks) = blocks::prepare(&self.view, bounds, window, cx);
        // A hitbox keeps the content mask it was inserted under, and hit
        // tests only its intersection with it. Rows, headers and expanders
        // cut by the viewport's edges (a header pushed up by the next one, a
        // gap row half scrolled off, an expander past the right edge) must
        // not take the pointer from the host's chrome around the viewport.
        let (cells, targets) = window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Code cells under the controls and the header strips.
            let cells = selection::insert_hitboxes(&frame, window);
            (cells, controls::insert_hitboxes(&frame, window))
        });
        Prepainted {
            frame: Some(frame),
            blocks,
            hitbox,
            targets,
            cells,
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
        let hovered = controls::hovered(&prepainted.targets, window);
        let hover = |layer: ControlLayer| {
            hovered
                .map(|i| controls::target_bounds(&prepainted.targets, i))
                .filter(|(_, l)| *l == layer)
                .map(|(b, _)| (b, frame.hover))
        };
        // The wheel handler goes first, so the handlers of the host blocks
        // and controls painted below run before it (bubble order is reverse
        // registration).
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
        // Presses on code cells, drags and the "+" hover: after the wheel,
        // before the blocks and controls, which get the pointer first.
        selection::wire(
            &self.view,
            prepainted.hitbox.clone(),
            prepainted.cells.clone(),
            window,
        );
        let (rows, headers) = frame.layers.split_at(HEADERS);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for layer in rows {
                paint_quads(layer, None, window);
            }
            if let Some((b, color)) = hover(ControlLayer::Body) {
                window.paint_quad(fill(b, color));
            }
            for layer in rows {
                paint_rounded(layer, window);
            }
            for layer in rows {
                paint_texts(layer, &frame, window, cx);
            }
            // Host blocks over the rows, under the headers.
            blocks::paint(&mut prepainted.blocks, window, cx);
            // Headers last: the pinned one covers the rows and blocks under
            // it.
            for layer in headers {
                paint_quads(layer, hover(ControlLayer::Header), window);
                paint_rounded(layer, window);
                paint_texts(layer, &frame, window, cx);
            }
        });
        controls::wire(&self.view, prepainted.targets.clone(), hovered, window);

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

/// A layer's plain quads, then `hover` (the highlighted control) on top of
/// them.
fn paint_quads(layer: &Layer, hover: Option<(Bounds<Pixels>, Hsla)>, window: &mut Window) {
    let clip = layer.clip.map(|bounds| ContentMask { bounds });
    window.with_content_mask(clip, |window| {
        for (bounds, color) in &layer.quads {
            window.paint_quad(fill(*bounds, *color));
        }
        if let Some((bounds, color)) = hover {
            window.paint_quad(fill(bounds, color));
        }
    });
}

fn paint_rounded(layer: &Layer, window: &mut Window) {
    if layer.rounded.is_empty() {
        return;
    }
    let clip = layer.clip.map(|bounds| ContentMask { bounds });
    window.with_content_mask(clip, |window| {
        for r in &layer.rounded {
            let (border_width, border_color) = match r.border {
                Some(color) => (px(1.), color),
                None => (px(0.), Hsla::transparent_black()),
            };
            window.paint_quad(quad(
                r.bounds,
                r.radius,
                r.background,
                border_width,
                border_color,
                BorderStyle::Solid,
            ));
        }
    });
}

fn paint_texts(layer: &Layer, frame: &Frame, window: &mut Window, cx: &mut App) {
    let clip = layer.clip.map(|bounds| ContentMask { bounds });
    window.with_content_mask(clip, |window| {
        for (origin, text) in &layer.texts {
            text.shaped.paint(*origin, frame.line_height, window, cx);
        }
    });
}
