//! Pass-through motion wrappers (ADR-0030 rules 7 and 13): no element id
//! and no layout node of their own (`request_layout` returns the child's),
//! so the child's layout, its id path and its element state (a scroll
//! offset, a pending click) are the same wrapped or not. Opacity is the
//! child's own root `Styled::opacity`.

use gpui_kit::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Point, Window,
};

/// `child` painted `offset` from where it is laid out; its hitboxes follow
/// the paint, its layout does not move.
pub fn slide(offset: Point<Pixels>, child: impl IntoElement) -> AnyElement {
    PassThrough {
        child: child.into_any_element(),
        how: How::Slide(offset),
    }
    .into_any_element()
}

/// `child` under a content mask: `mask` relative to the child's laid-out
/// origin (`Bounds::new(Point::default(), size)` clips to `size` from its
/// top-left).
pub fn clip(mask: Bounds<Pixels>, child: impl IntoElement) -> AnyElement {
    PassThrough {
        child: child.into_any_element(),
        how: How::Clip(mask),
    }
    .into_any_element()
}

enum How {
    Slide(Point<Pixels>),
    Clip(Bounds<Pixels>),
}

struct PassThrough {
    child: AnyElement,
    how: How,
}

impl PassThrough {
    /// The content mask in window coordinates, for a child laid out at
    /// `bounds`.
    fn mask(&self, bounds: Bounds<Pixels>) -> Option<ContentMask<Pixels>> {
        match self.how {
            How::Slide(_) => None,
            How::Clip(mask) => Some(ContentMask {
                bounds: Bounds::new(bounds.origin + mask.origin, mask.size),
            }),
        }
    }
}

impl IntoElement for PassThrough {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PassThrough {
    type RequestLayoutState = ();
    /// The content mask, in window coordinates.
    type PrepaintState = Option<ContentMask<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<ContentMask<Pixels>> {
        let mask = self.mask(bounds);
        let offset = match self.how {
            How::Slide(offset) => offset,
            How::Clip(_) => Point::default(),
        };
        window.with_content_mask(mask, |window| {
            window.with_element_offset(offset, |window| self.child.prepaint(window, cx))
        });
        mask
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        mask: &mut Option<ContentMask<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(*mask, |window| self.child.paint(window, cx));
    }
}
