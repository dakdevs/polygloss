//! The frame sentinel (T7.3, design §12.1): a full-app frame's draw time,
//! from the start of `MainWindow::render` ([`render_started`]) to the paint
//! of its last child ([`sentinel`]). `MainWindow`'s whole tree renders,
//! lays out and paints between the two; gpui-kit's root layers (dialogs,
//! toasts) paint after it and are not counted.
//!
//! Both halves are no-ops unless a [`FrameRecorder`] records that window:
//! the sentinel has no element id (so no element state), lays out an
//! absolutely placed empty node, paints nothing and never requests a frame.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, Global, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Position, Style, Window, WindowId,
};

/// One recorded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The render's start to the sentinel's paint, on the wall clock.
    pub draw: Duration,
    /// When the sentinel painted, on the executor clock (the wall clock in
    /// the app; what tests step).
    pub at: Instant,
}

#[derive(Default)]
struct Log {
    /// The start of the frame being drawn (wall clock).
    render_start: Option<Instant>,
    draws: Vec<Duration>,
    painted: Vec<Instant>,
}

/// The windows being recorded, by id.
#[derive(Default)]
struct Recordings(HashMap<WindowId, Weak<RefCell<Log>>>);

impl Global for Recordings {}

/// The log of `window`'s frames while a recorder of it is alive.
fn recording(window: &Window, cx: &App) -> Option<Rc<RefCell<Log>>> {
    cx.try_global::<Recordings>()?
        .0
        .get(&window.window_handle().window_id())?
        .upgrade()
}

/// Records every frame of one window until it is dropped (the latest
/// recorder of a window wins). Clones share the log.
#[derive(Clone)]
pub struct FrameRecorder {
    log: Rc<RefCell<Log>>,
}

impl FrameRecorder {
    pub fn start(window: &mut Window, cx: &mut App) -> Self {
        let log = Rc::new(RefCell::new(Log::default()));
        let recordings = &mut cx.default_global::<Recordings>().0;
        recordings.retain(|_, log| log.strong_count() > 0);
        recordings.insert(window.window_handle().window_id(), Rc::downgrade(&log));
        FrameRecorder { log }
    }

    /// Each recorded frame's draw time, oldest first. A `Ref` (not a slice
    /// borrowed from `self`): the sentinel appends while a recorder is held.
    pub fn frames(&self) -> Ref<'_, [Duration]> {
        Ref::map(self.log.borrow(), |l| l.draws.as_slice())
    }

    /// When each recorded frame painted, on the executor clock.
    pub fn painted_at(&self) -> Ref<'_, [Instant]> {
        Ref::map(self.log.borrow(), |l| l.painted.as_slice())
    }

    /// The frames recorded from index `from` on.
    pub fn since(&self, from: usize) -> Vec<Frame> {
        let log = self.log.borrow();
        log.draws
            .iter()
            .zip(&log.painted)
            .skip(from)
            .map(|(&draw, &at)| Frame { draw, at })
            .collect()
    }
}

/// Marks the start of `window`'s frame: first thing in its root view's
/// render. A no-op unless recording.
pub fn render_started(window: &Window, cx: &App) {
    if let Some(log) = recording(window, cx) {
        log.borrow_mut().render_start = Some(Instant::now());
    }
}

/// The sentinel: the root view's last child.
pub fn sentinel() -> AnyElement {
    Sentinel.into_any_element()
}

/// [`sentinel`]'s element.
pub struct Sentinel;

impl IntoElement for Sentinel {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Sentinel {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        // Out of the flow: its parent's layout is the same without it.
        let style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(log) = recording(window, cx) else {
            return;
        };
        let painted = Instant::now();
        let mut log = log.borrow_mut();
        if let Some(start) = log.render_start.take() {
            log.draws.push(painted.saturating_duration_since(start));
            log.painted.push(cx.background_executor().now());
        }
    }
}
