//! File cards on the canvas and the prelude (design §11.6, ADR-0027).
//!
//! Each file is a rounded card: the canvas shows around it, its rows fill
//! the card's inner width, its header sits inside it (in place it takes the
//! card's top corners; pinned it is square and flush at the top edge). The
//! prelude is a host element (the header card) above the first card, as wide
//! as a card, measured like a block and scrolling with the diff: it is the
//! first file's lead, so the document's top anchor keeps it at the top while
//! it loads or grows. The vertical geometry lives in the [`crate::Document`]
//! ([`crate::Document::lead`], [`crate::Document::header_top`],
//! [`crate::Document::card_bottom`]); this module paints it.

use gpui_kit::{Bounds, Context, Corners, Edges, Pixels, Point, point, px, size};

use crate::blocks::RenderBlock;
use crate::paint_rows::{FULL, HEADERS, Painter, RoundedQuad};
use crate::view::DiffViewport;

/// How file cards sit on the canvas ([`crate::ViewportOptions::cards`];
/// `None` there is the flat v1 layout: rows across the whole width, no
/// canvas, no gaps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardStyle {
    /// From each side of the viewport to a card's outer edge.
    pub margin_x: f32,
    /// Canvas between cards, below the last one, and between the prelude and
    /// the first one.
    pub gap: f32,
    pub radius: f32,
    /// Padding below a card's last row (none under an empty body).
    pub pad_bottom: f32,
    /// The card's border width.
    pub border: f32,
}

impl Default for CardStyle {
    /// Design §11.6: 16 pt from the sides, 12 pt apart, radius 8, 8 pt of
    /// padding, a 1 px border.
    fn default() -> CardStyle {
        CardStyle {
            margin_x: 16.0,
            gap: 12.0,
            radius: 8.0,
            pad_bottom: 8.0,
            border: 1.0,
        }
    }
}

/// How far rows sit from the viewport's left and right edges: the margin
/// and the border, 0 in the flat layout.
pub(crate) fn inset(cards: Option<CardStyle>) -> f32 {
    cards.map_or(0.0, |c| c.margin_x + c.border)
}

/// Where rows go in a viewport at `bounds`: inset on the left and right by
/// [`inset`].
pub(crate) fn inner_bounds(bounds: Bounds<Pixels>, cards: Option<CardStyle>) -> Bounds<Pixels> {
    let inset = px(inset(cards));
    let mut inner = bounds;
    inner.origin.x += inset;
    inner.size.width = (inner.size.width - inset * 2.0).max(px(0.));
    inner
}

/// Left edge and width of a card (borders included) in a viewport `width`
/// px wide: the whole width in the flat layout.
fn card_span(width: f32, cards: Option<CardStyle>) -> (f32, f32) {
    let margin = cards.map_or(0.0, |c| c.margin_x);
    (margin, (width - 2.0 * margin).max(0.0))
}

/// The host's prelude while set.
pub(crate) struct Prelude {
    pub render: RenderBlock,
    /// The width its current height was measured at (`None`: not yet, or
    /// set again since).
    pub measured: Option<f32>,
}

/// Where the prelude goes in a frame.
pub(crate) struct PreludeSlot {
    /// Top-left corner in window coordinates.
    pub origin: Point<Pixels>,
    pub width: f32,
    /// Its height in the document (what measuring checks).
    pub height: f32,
    /// Cut to the viewport.
    pub clip: Bounds<Pixels>,
    pub render: RenderBlock,
}

impl DiffViewport {
    /// Sets the prelude, or removes it with `None`: a host element above the
    /// first card (the header card, design §11.6), as wide as a card,
    /// measured like a block at that width and scrolling with the diff. A
    /// fresh document opens at its top, and an anchor at the top of the
    /// document stays there while it loads or grows. Setting it again keeps
    /// the last measured height until it is measured again (on the next
    /// frame). [`DiffViewport::set_provider`] keeps it.
    pub fn set_prelude(&mut self, render: Option<RenderBlock>, cx: &mut Context<Self>) {
        match render {
            Some(render) => {
                self.prelude = Some(Prelude {
                    render,
                    measured: None,
                });
                if self.doc.prelude_height().is_none() {
                    self.doc.set_prelude_height(Some(0.0));
                }
            }
            None => {
                self.prelude = None;
                self.doc.set_prelude_height(None);
            }
        }
        cx.notify();
    }

    /// The prelude's width: a card's.
    pub(crate) fn prelude_width(&self) -> f32 {
        card_span(self.outer_width, self.opts.cards).1
    }

    /// Stores the prelude's height measured at `width`. The anchor keeps
    /// what is on screen still.
    pub(crate) fn prelude_measured(&mut self, width: f32, height: f32) {
        let Some(prelude) = &mut self.prelude else {
            return;
        };
        prelude.measured = Some(width);
        self.doc.set_prelude_height(Some(height));
    }

    /// The prelude to measure off screen: set, but not measured at the
    /// current width.
    pub(crate) fn prelude_to_measure(&self) -> Option<(RenderBlock, f32)> {
        let prelude = self.prelude.as_ref()?;
        let width = self.prelude_width();
        (prelude.measured != Some(width)).then(|| (prelude.render.clone(), width))
    }
}

impl Painter<'_> {
    /// Left edge and width of the rows, relative to the viewport: the card's
    /// inner width (the whole width in the flat layout).
    pub(crate) fn inner_x_w(&self) -> (f32, f32) {
        let x = (self.inner.origin.x - self.bounds.origin.x).as_f32();
        (x, self.inner.size.width.as_f32())
    }

    /// Left edge and width of a card, borders included.
    pub(crate) fn card_x_w(&self) -> (f32, f32) {
        card_span(self.bounds.size.width.as_f32(), self.cards)
    }

    /// What is behind the rows: the viewport's background in the flat
    /// layout; with cards, the canvas and one rounded quad per visible card
    /// (their own layer), each cut to the viewport plus its corners and a
    /// little, so a long card never makes a huge quad.
    pub(crate) fn paint_canvas(&mut self, visible: std::ops::Range<u32>) {
        let b = self.bounds;
        let (width, height) = (b.size.width.as_f32(), b.size.height.as_f32());
        let Some(style) = self.cards else {
            self.quad(FULL, 0.0, 0.0, width, height, self.theme.background);
            return;
        };
        let layer = &mut self.frame.cards;
        layer.clip = Some(b);
        layer.quads.push((b, self.theme.canvas));
        let (x, w) = self.card_x_w();
        let reach = style.radius + 2.0;
        for f in visible {
            let top = (self.doc.header_top(f) - self.scroll_top) as f32;
            let bottom = (self.doc.card_bottom(f) - self.scroll_top) as f32;
            let (top, bottom) = (top.max(-reach), bottom.min(height + reach));
            if bottom <= top {
                continue;
            }
            let bounds = self.bounds_at(x, top, w, bottom - top);
            self.frame.cards.rounded.push(RoundedQuad {
                bounds,
                background: self.theme.card_background,
                border: Some(self.theme.card_border),
                border_widths: Edges::all(px(style.border)),
                radius: Corners::all(px(style.radius)),
            });
        }
    }

    /// The header's strip and the area that takes the clicks over the rows
    /// it covers. Flat: across the viewport with a top and a bottom border.
    /// On a card: across the card; in place it has the card's border and top
    /// corners (all four when the card is only its header), pinned
    /// (`sticky`) it is square with a bottom border.
    pub(crate) fn header_strip(&mut self, f: u32, y: f32, h: f32, sticky: bool) {
        let theme = self.theme;
        let Some(style) = self.cards else {
            let width = self.bounds.size.width.as_f32();
            self.quad(HEADERS, 0.0, y, width, h, theme.header_background);
            self.quad(HEADERS, 0.0, y, width, 1.0, theme.border);
            self.quad(HEADERS, 0.0, y + h - 1.0, width, 1.0, theme.border);
            let area = self.bounds_at(0.0, y, width, h);
            self.frame.header_areas.push(area);
            return;
        };
        let (x, w) = self.card_x_w();
        let bounds = self.bounds_at(x, y, w, h);
        let (r, b) = (px(style.radius), px(style.border));
        let (radius, border_widths) = if sticky {
            (
                Corners::all(px(0.)),
                Edges {
                    top: px(0.),
                    right: b,
                    bottom: b,
                    left: b,
                },
            )
        } else {
            let bottom = if self.doc.body_height(f) > 0.0 {
                px(0.)
            } else {
                r
            };
            let radius = Corners {
                top_left: r,
                top_right: r,
                bottom_right: bottom,
                bottom_left: bottom,
            };
            (radius, Edges::all(b))
        };
        self.frame.layers[HEADERS].rounded.push(RoundedQuad {
            bounds,
            background: theme.header_background,
            border: Some(theme.card_border),
            border_widths,
            radius,
        });
        self.frame.header_areas.push(bounds);
    }

    /// Places the prelude at the top of the document, at a card's width, when
    /// it reaches into the viewport (a prelude not measured yet is 0 px tall
    /// and placed at the top, so it gets measured).
    pub(crate) fn place_prelude(&mut self) {
        let Some(render) = self.prelude else {
            return;
        };
        let h = self.doc.prelude_height().unwrap_or(0.0);
        let y = -self.scroll_top as f32;
        if y + h < 0.0 || y > self.bounds.size.height.as_f32() {
            return;
        }
        let (x, w) = self.card_x_w();
        let o = self.bounds.origin;
        let origin = point(o.x + px(x), o.y + px(y));
        self.frame.prelude = Some(PreludeSlot {
            origin,
            width: w,
            height: h,
            clip: Bounds::new(origin, size(px(w), px(h))).intersect(&self.bounds),
            render: render.clone(),
        });
    }
}
