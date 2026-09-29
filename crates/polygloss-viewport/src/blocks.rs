//! Host blocks: variable-height elements (threads, composers, notes) below
//! their anchored line (design §11.6 "Threads", §12.4 "Blocks").
//!
//! Blocks are opaque to the viewport: the host gives each one an id, an
//! anchor and a render function, and [`DiffViewport::set_blocks`] hands them
//! to the [`crate::Document`], which places them as rows of their own
//! ([`crate::FileLayout::with_blocks`]) and re-lays out only that file. In
//! split a block sits in its side's column with a same-height spacer on the
//! other side; in unified, and for file-level blocks, it spans the full width.
//!
//! Heights are measured, never guessed for long: every frame renders the
//! visible blocks (GPUI elements live for one frame), lays them out at their
//! column's width and corrects their rows when the measured height differs,
//! rebuilding the frame so the first painted frame is already right. Blocks
//! near the viewport whose height is unknown (new, invalidated, or measured at
//! another width) are measured too, a few per frame, so they are exact before
//! they scroll in. The scroll anchor keeps visible content still through all
//! of it; a block the viewport's top edge cuts that was not on screen before
//! keeps its bottom edge still instead, so scrolling up into it never moves
//! the rows below it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Context, ElementId, Entity, Pixels,
    Point, Window, point, px, size,
};
use polygloss_diff::Side;
use polygloss_diff::rows::Layout;

pub use crate::document::{BlockAnchor, BlockId, PlacedBlock};
use crate::document::{BodyRow, DEFAULT_WINDOW_SCREENS, RowKey, ScrollAnchor};
use crate::layout::Pane;
#[cfg(feature = "debug-inspect")]
use crate::paint_rows::{DebugContent, DebugRow};
use crate::paint_rows::{Frame, Painter};
use crate::view::DiffViewport;

/// Height of a block before it is first measured, in code rows.
pub const ESTIMATED_BLOCK_ROWS: f32 = 3.0;

/// Times a frame is built at most while its visible blocks are measured
/// (each pass corrects the blocks the previous one found mis-sized).
const BLOCK_PASSES: usize = 3;

/// Blocks outside the viewport measured per frame at most.
const MAX_OFFSCREEN_MEASURES: usize = 8;

/// Renders a block. Called on the main thread for every frame the block is
/// visible (or about to be measured); the element lives for that frame only.
pub type RenderBlock = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A host block: a thread, composer or note, opaque to the viewport.
#[derive(Clone)]
pub struct BlockSpec {
    /// Unique across the viewport. Reusing an id in another file moves the
    /// block there.
    pub id: BlockId,
    pub anchor: BlockAnchor,
    pub render: RenderBlock,
}

impl fmt::Debug for BlockSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockSpec")
            .field("id", &self.id)
            .field("anchor", &self.anchor)
            .finish_non_exhaustive()
    }
}

/// The viewport's host blocks: their specs by id, and what is known about
/// their heights. Placement and heights live in the document.
#[derive(Default)]
pub(crate) struct Blocks {
    specs: HashMap<BlockId, (u32, BlockSpec)>,
    /// The column width each block's current height was measured at. Missing
    /// when it was never measured or was invalidated.
    measured: HashMap<BlockId, f32>,
    /// Blocks the last frame painted.
    painted: Vec<BlockId>,
}

impl Blocks {
    /// The file block `id` is in.
    pub(crate) fn file_of(&self, id: BlockId) -> Option<u32> {
        self.specs.get(&id).map(|(f, _)| *f)
    }

    fn spec(&self, id: BlockId) -> Option<&BlockSpec> {
        self.specs.get(&id).map(|(_, spec)| spec)
    }
}

/// Where a block goes in a row: its side's column in split, else the whole
/// width.
fn block_pane(anchor: BlockAnchor, layout: Layout) -> Pane {
    match (layout, anchor) {
        (
            Layout::Split,
            BlockAnchor::Line {
                side: Side::Old, ..
            },
        ) => Pane::Half(0),
        (
            Layout::Split,
            BlockAnchor::Line {
                side: Side::New, ..
            },
        ) => Pane::Half(1),
        _ => Pane::Full,
    }
}

/// Left edge and width of a block in `pane` of a viewport `width` px wide. A
/// left-column block stops before the divider (the left half's last pixel
/// column), which keeps running between the columns.
fn block_column(pane: Pane, width: f32) -> (f32, f32) {
    let half = (width / 2.0).floor();
    match pane {
        Pane::Full => (0.0, width),
        Pane::Half(0) => (0.0, (half - 1.0).max(0.0)),
        Pane::Half(_) => (half, (width - half).max(0.0)),
    }
}

/// A visible block row in a frame: where its element goes.
pub(crate) struct BlockSlot {
    pub id: BlockId,
    pub file: u32,
    /// Top-left corner in window coordinates.
    pub origin: Point<Pixels>,
    pub width: f32,
    /// The row's height in the layout.
    pub height: f32,
    /// The row, cut to the viewport: the element never paints outside it.
    pub clip: Bounds<Pixels>,
    pub render: RenderBlock,
}

impl<'a> Painter<'a> {
    /// A block row: queues its element's slot and, in split, the spacer on
    /// the other side.
    pub(crate) fn block(&mut self, f: u32, id: BlockId, y: f32, h: f32) {
        #[cfg(feature = "debug-inspect")]
        self.debug.push(DebugRow {
            y,
            height: h,
            styled: false,
            content: DebugContent::Block(id.0),
        });
        let blocks: &'a Blocks = self.blocks;
        let Some(spec) = blocks.spec(id) else {
            return;
        };
        let width = self.bounds.size.width.as_f32();
        let pane = block_pane(spec.anchor, self.layout);
        if let Pane::Half(k) = pane {
            let half = (width / 2.0).floor();
            let theme = self.theme;
            if k == 0 {
                self.quad(2, half, y, width - half, h, theme.empty_cell);
            } else {
                self.quad(1, 0.0, y, half, h, theme.empty_cell);
            }
            self.quad(1, half - 1.0, y, 1.0, h, theme.border);
        }
        let (x, w) = block_column(pane, width);
        let o = self.bounds.origin;
        let origin = point(o.x + px(x), o.y + px(y));
        let render = spec.render.clone();
        let clip = Bounds::new(origin, size(px(w), px(h))).intersect(&self.bounds);
        self.frame.blocks.push(BlockSlot {
            id,
            file: f,
            origin,
            width: w,
            height: h,
            clip,
            render,
        });
    }
}

/// A block's measured height at a width.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Measured {
    file: u32,
    id: BlockId,
    width: f32,
    height: f32,
}

/// A block near the viewport to measure.
struct ToMeasure {
    file: u32,
    id: BlockId,
    width: f32,
    render: RenderBlock,
}

impl DiffViewport {
    /// Replaces file `file_idx`'s blocks (design §12.4 "Blocks"). Only that
    /// file is laid out again (and a file an id moved away from). Blocks keep
    /// the height measured for their id; new ones start estimated and are
    /// measured on the next frame when near the viewport. When a block's
    /// content changes, call [`DiffViewport::invalidate_block`]. Duplicate ids
    /// keep the first. Nothing on screen moves above the new blocks.
    pub fn set_blocks(&mut self, file_idx: u32, blocks: Vec<BlockSpec>, cx: &mut Context<Self>) {
        if file_idx >= self.doc.len() {
            return;
        }
        let keep: HashSet<BlockId> = blocks.iter().map(|b| b.id).collect();
        for old in self.doc.blocks(file_idx) {
            if !keep.contains(&old.id) {
                self.blocks.specs.remove(&old.id);
                self.blocks.measured.remove(&old.id);
            }
        }
        let estimate = ESTIMATED_BLOCK_ROWS * self.doc.metrics().row_height;
        let mut seen = HashSet::with_capacity(blocks.len());
        let mut placed = Vec::with_capacity(blocks.len());
        let mut moved_from = Vec::new();
        for spec in blocks {
            if !seen.insert(spec.id) {
                continue;
            }
            let height = match self.blocks.file_of(spec.id) {
                Some(g) => {
                    if g != file_idx {
                        moved_from.push(g);
                    }
                    self.doc
                        .blocks(g)
                        .iter()
                        .find(|b| b.id == spec.id)
                        .map_or(estimate, |b| b.height)
                }
                None => estimate,
            };
            placed.push(PlacedBlock {
                id: spec.id,
                anchor: spec.anchor,
                height,
            });
            self.blocks.specs.insert(spec.id, (file_idx, spec));
        }
        moved_from.sort_unstable();
        moved_from.dedup();
        for g in moved_from {
            let rest = self
                .doc
                .blocks(g)
                .iter()
                .filter(|b| self.blocks.file_of(b.id) == Some(g))
                .copied()
                .collect();
            self.doc.set_blocks(g, rest);
        }
        self.doc.set_blocks(file_idx, placed);
        cx.notify();
    }

    /// Block `id`'s content changed: it is measured again on the next frame
    /// if it is visible or near the viewport (otherwise when it gets there).
    pub fn invalidate_block(&mut self, id: BlockId, cx: &mut Context<Self>) {
        self.blocks.measured.remove(&id);
        if self.blocks.specs.contains_key(&id) {
            cx.notify();
        }
    }

    /// Stores measured block heights. A height change keeps the scroll
    /// anchor, except that a block cut by the viewport's top edge that the
    /// last frame did not show keeps its bottom edge still (the rows below it
    /// were on screen, it was not).
    fn record_block_heights(&mut self, measured: &[Measured]) {
        for m in measured {
            self.blocks.measured.insert(m.id, m.width);
            let Some(old) = self
                .doc
                .blocks(m.file)
                .iter()
                .find(|b| b.id == m.id)
                .map(|b| b.height)
            else {
                continue;
            };
            if old == m.height {
                continue;
            }
            let anchor = *self.doc.anchor();
            let keep_bottom = anchor.file_idx == m.file
                && anchor.row == RowKey::Block(m.id)
                && anchor.offset_px > 0.0
                && !self.blocks.painted.contains(&m.id);
            self.doc.set_block_height(m.file, m.id, m.height);
            if keep_bottom {
                self.doc.scroll_to_anchor(ScrollAnchor {
                    offset_px: (anchor.offset_px + m.height - old).max(0.0),
                    ..anchor
                });
            }
        }
    }

    /// Blocks within the materialization band around the viewport, not in
    /// `visible`, whose height is not known at their current width: at most
    /// `max`, and whether more are left.
    fn blocks_to_measure(&self, visible: &[BlockSlot], max: usize) -> (Vec<ToMeasure>, bool) {
        let mut out = Vec::new();
        let h = self.doc.viewport_height();
        if h <= 0.0 || self.blocks.specs.is_empty() {
            return (out, false);
        }
        let margin = f64::from(DEFAULT_WINDOW_SCREENS * h);
        let top = self.doc.scroll_top() - margin;
        let bottom = self.doc.scroll_top() + f64::from(h) + margin;
        let header = f64::from(self.doc.metrics().header_height);
        for f in self.doc.materialize_range(h, DEFAULT_WINDOW_SCREENS) {
            if self.doc.blocks(f).is_empty() || self.doc.is_collapsed(f) {
                continue;
            }
            let Some(layout) = self.doc.file_layout(f) else {
                continue;
            };
            let body_top = self.doc.file_top(f) + header;
            for &r in layout.block_rows() {
                let r = r as usize;
                let y = body_top + layout.row_top(r);
                if y >= bottom {
                    break;
                }
                if y + f64::from(layout.row_height(r)) <= top {
                    continue;
                }
                let BodyRow::Block(id) = layout.rows()[r] else {
                    continue;
                };
                if visible.iter().any(|s| s.id == id) {
                    continue;
                }
                let Some(spec) = self.blocks.spec(id) else {
                    continue;
                };
                let (_, width) = block_column(block_pane(spec.anchor, self.layout), self.width);
                if self.blocks.measured.get(&id) == Some(&width) {
                    continue;
                }
                if out.len() == max {
                    return (out, true);
                }
                out.push(ToMeasure {
                    file: f,
                    id,
                    width,
                    render: spec.render.clone(),
                });
            }
        }
        (out, false)
    }

    fn finish_block_frame(&mut self, frame: &Frame) {
        self.blocks.painted.clear();
        self.blocks
            .painted
            .extend(frame.blocks.iter().map(|slot| slot.id));
    }
}

/// A block element laid out at a width.
struct Laid {
    id: BlockId,
    width: f32,
    height: f32,
    element: AnyElement,
}

/// A visible block's element, prepainted and ready to paint.
pub(crate) struct PreparedBlock {
    id: BlockId,
    element: AnyElement,
    clip: Bounds<Pixels>,
}

/// Every block element runs under its own id, so hosts need not make
/// element ids unique across blocks.
fn element_id(id: BlockId) -> ElementId {
    ElementId::from(("polygloss-block", id.0))
}

/// Renders block `id` and lays it out `width` px wide; its height is rounded
/// up to whole pixels so the rows below stay on the pixel grid.
fn measure(
    id: BlockId,
    render: &RenderBlock,
    width: f32,
    window: &mut Window,
    cx: &mut App,
) -> Laid {
    window.with_id(element_id(id), |window| {
        let mut element = render(window, cx);
        let available = size(
            AvailableSpace::Definite(px(width)),
            AvailableSpace::MinContent,
        );
        let height = element
            .layout_as_root(available, window, cx)
            .height
            .as_f32();
        Laid {
            id,
            width,
            height: if height.is_finite() {
                height.ceil().max(0.0)
            } else {
                0.0
            },
            element,
        }
    })
}

/// The element's prepaint: builds the frame (see
/// [`DiffViewport::prepare_frame`]), renders and measures its visible blocks,
/// rebuilds it while block heights needed correcting, prepaints the blocks at
/// their rows and measures blocks near the viewport.
pub(crate) fn prepare(
    view: &Entity<DiffViewport>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> (Frame, Vec<PreparedBlock>) {
    let mut earlier: Vec<Laid> = Vec::new();
    let mut shaped = 0;
    let mut pass = 1;
    loop {
        let mut frame = view.update(cx, |v, cx| v.prepare_frame(bounds, window, cx));
        let mut laid = Vec::with_capacity(frame.blocks.len());
        let mut measured = Vec::with_capacity(frame.blocks.len());
        let mut stale = false;
        for slot in &frame.blocks {
            // An element an earlier pass laid out at this width is reused.
            let l = match earlier
                .iter()
                .position(|l| l.id == slot.id && l.width == slot.width)
            {
                Some(i) => earlier.swap_remove(i),
                None => measure(slot.id, &slot.render, slot.width, window, cx),
            };
            stale |= l.height != slot.height;
            measured.push(Measured {
                file: slot.file,
                id: slot.id,
                width: slot.width,
                height: l.height,
            });
            laid.push(l);
        }
        if !measured.is_empty() {
            view.update(cx, |v, _| v.record_block_heights(&measured));
        }
        if !stale || pass == BLOCK_PASSES {
            if stale {
                // Out of passes: this frame shows the old heights, the next
                // one the measured ones.
                request_frame(view, window);
            }
            frame.shaped += shaped;
            let prepared = frame
                .blocks
                .iter()
                .zip(laid)
                .map(|(slot, mut l)| {
                    window.with_id(element_id(slot.id), |window| {
                        window.with_content_mask(Some(ContentMask { bounds: slot.clip }), |w| {
                            l.element.prepaint_at(slot.origin, w, cx)
                        })
                    });
                    PreparedBlock {
                        id: slot.id,
                        element: l.element,
                        clip: slot.clip,
                    }
                })
                .collect();
            measure_nearby(view, &frame, window, cx);
            if !frame.blocks.is_empty() || !view.read(cx).blocks.painted.is_empty() {
                view.update(cx, |v, _| v.finish_block_frame(&frame));
            }
            return (frame, prepared);
        }
        // Heights changed (the anchor kept visible content still): build the
        // frame again with them.
        shaped += frame.shaped;
        view.update(cx, |v, _| v.frame_pool = Some(frame));
        earlier = laid;
        pass += 1;
    }
}

/// Measures blocks near the viewport whose height is unknown, a few per
/// frame. They are off screen, so nothing painted this frame moves.
fn measure_nearby(view: &Entity<DiffViewport>, frame: &Frame, window: &mut Window, cx: &mut App) {
    let (todo, more) = view
        .read(cx)
        .blocks_to_measure(&frame.blocks, MAX_OFFSCREEN_MEASURES);
    if todo.is_empty() {
        return;
    }
    let measured: Vec<Measured> = todo
        .into_iter()
        .map(|t| {
            let l = measure(t.id, &t.render, t.width, window, cx);
            Measured {
                file: t.file,
                id: t.id,
                width: t.width,
                height: l.height,
            }
        })
        .collect();
    view.update(cx, |v, _| v.record_block_heights(&measured));
    if more {
        request_frame(view, window);
    }
}

fn request_frame(view: &Entity<DiffViewport>, window: &mut Window) {
    let view = view.entity_id();
    window.on_next_frame(move |_, cx| cx.notify(view));
}

/// Paints the prepared blocks, each clipped to its row.
pub(crate) fn paint(blocks: &mut [PreparedBlock], window: &mut Window, cx: &mut App) {
    for b in blocks {
        window.with_id(element_id(b.id), |window| {
            window.with_content_mask(Some(ContentMask { bounds: b.clip }), |window| {
                b.element.paint(window, cx)
            })
        });
    }
}
