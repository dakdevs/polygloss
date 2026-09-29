//! Clickable controls the viewport paints itself (design §11.6): the header's
//! collapse chevron, Viewed checkbox and ⋯ menu button, gap expanders and
//! "Load diff".
//!
//! The painter records each control's bounds and action in the frame. The
//! element turns them into hitboxes: body controls first, then every header's
//! whole strip (blocking the mouse but not the scroll wheel, so a pinned
//! header takes the clicks meant for rows under it), then the header
//! controls, all clipped to the viewport. Paint highlights the control under
//! the pointer, sets the pointer cursor, and a press followed by a release on
//! the same control activates it (pressing ⋯ while its menu is open closes
//! the menu).

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    App, Bounds, Context, CursorStyle, DispatchPhase, Entity, Hitbox, HitboxBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window,
};
use polygloss_diff::rows::{ExpandBy, GapId};

use crate::paint_rows::Frame;
use crate::view::{DiffViewport, ViewportEvent};

/// What a control does when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlAction {
    /// The header's chevron: collapse or expand the file.
    Collapse(u32),
    /// The header's Viewed checkbox: emits [`ViewportEvent::ViewedToggled`]
    /// (Viewed is the host's state, pushed back with
    /// [`DiffViewport::set_file_flags`]).
    Viewed(u32),
    /// The header's ⋯ button: opens the file menu.
    Menu(u32),
    /// A gap expander ("↑ 20", "↓ 20", "Expand all").
    Expand {
        file_idx: u32,
        gap: GapId,
        by: ExpandBy,
    },
    /// "Load diff" on a large or generated file: loads it and emits
    /// [`ViewportEvent::LoadDiffRequested`].
    LoadDiff(u32),
}

/// Which pass a control is painted in: with the rows, or in the header
/// overlay on top of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlLayer {
    Body,
    Header,
}

/// A control recorded by the painter.
#[derive(Debug, Clone)]
pub(crate) struct Control {
    pub action: ControlAction,
    /// Window coordinates.
    pub bounds: Bounds<Pixels>,
    pub layer: ControlLayer,
    /// A gap expander's hidden run (old lines): a click acts on the run it
    /// was painted on, also when reveals split its gap into several runs.
    pub run: Option<Range<u32>>,
}

impl Control {
    /// Whether `other` is the same control: the same action on the same
    /// hidden run (two runs of one gap share their expanders' actions).
    fn same_as(&self, other: &Pressed) -> bool {
        self.action == other.action && self.run == other.run
    }
}

/// The control a left button went down on; a release activates it only on
/// the same control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pressed {
    action: ControlAction,
    run: Option<Range<u32>>,
}

/// A control's hitbox, inserted during prepaint.
pub(crate) struct Target {
    hitbox: Hitbox,
    control: Control,
}

/// Inserts the frame's hitboxes in stacking order and returns the controls'.
pub(crate) fn insert_hitboxes(frame: &Frame, window: &mut Window) -> Rc<[Target]> {
    let mut targets = Vec::with_capacity(frame.controls.len());
    let mut insert = |layer: ControlLayer, window: &mut Window| {
        for control in frame.controls.iter().filter(|c| c.layer == layer) {
            targets.push(Target {
                hitbox: window.insert_hitbox(control.bounds, HitboxBehavior::Normal),
                control: control.clone(),
            });
        }
    };
    insert(ControlLayer::Body, window);
    for area in &frame.header_areas {
        window.insert_hitbox(*area, HitboxBehavior::BlockMouseExceptScroll);
    }
    insert(ControlLayer::Header, window);
    targets.into()
}

/// The control under the pointer, if any.
pub(crate) fn hovered(targets: &[Target], window: &Window) -> Option<usize> {
    targets.iter().position(|t| t.hitbox.is_hovered(window))
}

/// Bounds and layer of target `i`.
pub(crate) fn target_bounds(targets: &[Target], i: usize) -> (Bounds<Pixels>, ControlLayer) {
    let c = &targets[i].control;
    (c.bounds, c.layer)
}

/// Sets the pointer cursor over every control and wires press, release and
/// hover tracking. `hovered` is the control highlighted by this frame.
pub(crate) fn wire(
    view: &Entity<DiffViewport>,
    targets: Rc<[Target]>,
    hovered: Option<usize>,
    window: &mut Window,
) {
    for target in targets.iter() {
        window.set_cursor_style(CursorStyle::PointingHand, &target.hitbox);
    }
    if targets.is_empty() {
        return;
    }
    let (press_view, press_targets) = (view.clone(), targets.clone());
    window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
            return;
        }
        if let Some(t) = press_targets.iter().find(|t| t.hitbox.is_hovered(window)) {
            let control = &t.control;
            press_view.update(cx, |v, cx| {
                // ⋯ on its own open menu closes it; the release must not
                // open it again.
                let open = v.menu.as_ref().map(|m| m.file_idx);
                if matches!(control.action, ControlAction::Menu(f) if open == Some(f)) {
                    v.close_menu(cx);
                    v.pressed = None;
                } else {
                    v.pressed = Some(Pressed {
                        action: control.action,
                        run: control.run.clone(),
                    });
                }
            });
            cx.stop_propagation();
        }
    });
    let (release_view, release_targets) = (view.clone(), targets.clone());
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
            return;
        }
        let pressed = release_view.update(cx, |v, _| v.pressed.take());
        let Some(t) = release_targets.iter().find(|t| t.hitbox.is_hovered(window)) else {
            return;
        };
        if pressed.is_some_and(|p| t.control.same_as(&p)) {
            let control = t.control.clone();
            release_view.update(cx, |v, cx| v.activate(&control, window, cx));
            cx.stop_propagation();
        }
    });
    let hover_view = view.entity_id();
    window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx: &mut App| {
        if phase == DispatchPhase::Bubble
            && targets.iter().position(|t| t.hitbox.is_hovered(window)) != hovered
        {
            cx.notify(hover_view);
        }
    });
}

impl DiffViewport {
    /// Performs a clicked control's action.
    pub(crate) fn activate(
        &mut self,
        control: &Control,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match control.action {
            ControlAction::Collapse(f) => {
                let collapsed = self.doc.is_collapsed(f);
                self.set_collapsed(f, !collapsed, cx);
            }
            ControlAction::Viewed(f) => cx.emit(ViewportEvent::ViewedToggled(f)),
            ControlAction::Menu(f) => self.open_menu(f, control.bounds, window, cx),
            ControlAction::Expand { file_idx, gap, by } => match &control.run {
                Some(run) => self.expand_run(file_idx, run.clone(), by, cx),
                None => self.expand(file_idx, gap, by, cx),
            },
            ControlAction::LoadDiff(f) => {
                self.load_diff(f, cx);
                cx.emit(ViewportEvent::LoadDiffRequested(f));
            }
        }
    }
}
