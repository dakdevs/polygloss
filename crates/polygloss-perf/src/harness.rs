//! The GPUI side of a run: one headed window holding a [`DiffViewport`], a
//! log of every frame it paints, and a per-frame scroll driver.
//!
//! Frames are reported by the viewport itself (`ViewportEvent::FrameStats`,
//! emitted at the end of paint) and stamped with [`Instant::now`] when they
//! arrive, so frame-to-frame intervals include the main-thread work between
//! frames that `FrameStats` does not time (loads applied, token swap-in,
//! eviction scans). Scrolling is driven from `Window::on_next_frame` (what
//! `request_animation_frame` is built on): every display frame first moves
//! the viewport by `velocity × elapsed` and then draws, so the scripted speed
//! holds whatever the frame rate. The window never throttles itself while
//! inactive, so a run does not slow down when another app takes focus.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use futures::channel::{mpsc, oneshot};
use futures::future::{Either, select};
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Context, Entity, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, px, size,
};
use polygloss_viewport::{
    DiffProvider, DiffViewport, FrameStats, ScrollTarget, ViewportEvent, ViewportOptions,
};

use crate::scenarios::scroll::Scroller;

/// The window's content size in points (the gate shell's size).
pub const WINDOW_SIZE: (f32, f32) = (1440.0, 900.0);

/// One painted frame and when its stats arrived.
#[derive(Debug, Clone, Copy)]
pub struct FrameRecord {
    pub at: Instant,
    pub stats: FrameStats,
}

impl FrameRecord {
    /// Prepaint + paint CPU time (`scroll_p95_ms` samples).
    pub fn cpu(&self) -> Duration {
        self.stats.prepaint + self.stats.paint
    }
}

/// The window's root view: the viewport, filling it.
pub struct PerfShell {
    viewport: Entity<DiffViewport>,
    _frames: Subscription,
}

impl Render for PerfShell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().size_full().child(self.viewport.clone())
    }
}

/// What the per-frame driver does: at most one scroll at a time.
#[derive(Default)]
struct Driver {
    /// Bumped by every [`Harness::scroll`], so a replaced scroll's frame
    /// callbacks stop.
    generation: u64,
    scroller: Option<Scroller>,
    /// Told the last scroll step's time when the scroll ends.
    done: Option<oneshot::Sender<Option<Instant>>>,
}

/// A run's window, its frame log and its scroll driver.
pub struct Harness {
    pub viewport: Entity<DiffViewport>,
    window: AnyWindowHandle,
    frames: mpsc::UnboundedReceiver<FrameRecord>,
    /// Every frame taken from the log so far, in order.
    pub seen: Vec<FrameRecord>,
    driver: Rc<RefCell<Driver>>,
    /// When the window was opened.
    pub opened_at: Instant,
}

impl Harness {
    /// Opens the window (centered, focused, `WINDOW_SIZE`) over `provider`.
    pub fn open(
        cx: &mut App,
        provider: Arc<dyn DiffProvider>,
        options: ViewportOptions,
        title: &str,
    ) -> anyhow::Result<Harness> {
        let (tx, frames) = mpsc::unbounded();
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(
                size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
                cx,
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(title.to_owned().into()),
                ..TitlebarOptions::default()
            }),
            focus: true,
            show: true,
            inactive_frame_interval: None,
            ..WindowOptions::default()
        };
        let opened_at = Instant::now();
        let handle = cx.open_window(window_options, |window, cx| {
            let viewport = cx.new(|cx| DiffViewport::new(provider, options, window, cx));
            let frames = cx.subscribe(&viewport, move |_, event: &ViewportEvent, _| {
                if let ViewportEvent::FrameStats(stats) = event {
                    let _ = tx.unbounded_send(FrameRecord {
                        at: Instant::now(),
                        stats: *stats,
                    });
                }
            });
            cx.new(|_| PerfShell {
                viewport,
                _frames: frames,
            })
        })?;
        let viewport = handle.update(cx, |shell, _, _| shell.viewport.clone())?;
        Ok(Harness {
            viewport,
            window: handle.into(),
            frames,
            seen: Vec::new(),
            driver: Rc::default(),
            opened_at,
        })
    }

    /// The window's content size in points and its scale factor.
    pub fn window_metrics(&self, cx: &mut AsyncApp) -> anyhow::Result<(f32, f32, f32)> {
        cx.update_window(self.window, |_, window, _| {
            let size = window.viewport_size();
            (
                size.width.as_f32(),
                size.height.as_f32(),
                window.scale_factor(),
            )
        })
    }

    /// Reads the viewport.
    pub fn read<R>(&self, cx: &AsyncApp, f: impl FnOnce(&DiffViewport) -> R) -> R {
        self.viewport.read_with(cx, |v, _| f(v))
    }

    /// Updates the viewport (a scroll, blocks).
    pub fn update<R>(
        &self,
        cx: &mut AsyncApp,
        f: impl FnOnce(&mut DiffViewport, &mut Context<DiffViewport>) -> R,
    ) -> R {
        self.viewport.update(cx, f)
    }

    /// The next frame, or `None` after `timeout` without one.
    pub async fn next_frame(
        &mut self,
        cx: &AsyncApp,
        timeout: Duration,
    ) -> anyhow::Result<Option<FrameRecord>> {
        let timer = cx.background_executor().timer(timeout);
        match select(self.frames.next(), timer).await {
            Either::Left((Some(frame), _)) => {
                self.seen.push(frame);
                Ok(Some(frame))
            }
            Either::Left((None, _)) => anyhow::bail!("the window closed"),
            Either::Right(_) => Ok(None),
        }
    }

    /// The first frame (from now on) that `pred` accepts, or `None` when
    /// `timeout` passes first.
    pub async fn wait_for(
        &mut self,
        cx: &AsyncApp,
        timeout: Duration,
        pred: impl Fn(&FrameRecord) -> bool,
    ) -> anyhow::Result<Option<FrameRecord>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            match self.next_frame(cx, left).await? {
                Some(frame) if pred(&frame) => return Ok(Some(frame)),
                Some(_) => {}
                None => return Ok(None),
            }
        }
    }

    /// Waits until no frame arrives for `period` (at most `cap`); says
    /// whether it got quiet.
    pub async fn quiet(
        &mut self,
        cx: &AsyncApp,
        period: Duration,
        cap: Duration,
    ) -> anyhow::Result<bool> {
        let deadline = Instant::now() + cap;
        while Instant::now() < deadline {
            if self.next_frame(cx, period).await?.is_none() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Every frame that arrived and was not taken yet.
    pub fn drain(&mut self) -> Vec<FrameRecord> {
        let mut out = Vec::new();
        while let Ok(frame) = self.frames.try_recv() {
            out.push(frame);
        }
        self.seen.extend_from_slice(&out);
        out
    }

    /// Scrolls at `velocity` px/s (negative: up; turning around at the
    /// document's ends) for `duration`, one step per display frame. The
    /// receiver gets the time of the last step (the last scroll event) once
    /// the scroll has ended. Replaces a scroll still running.
    pub fn scroll(
        &self,
        cx: &mut AsyncApp,
        velocity: f32,
        duration: Duration,
    ) -> anyhow::Result<oneshot::Receiver<Option<Instant>>> {
        let (tx, rx) = oneshot::channel();
        let now = Instant::now();
        let generation = {
            let mut driver = self.driver.borrow_mut();
            driver.generation += 1;
            driver.scroller = Some(Scroller::new(velocity, now, now + duration));
            driver.done = Some(tx);
            driver.generation
        };
        let driver = self.driver.clone();
        let viewport = self.viewport.clone();
        cx.update_window(self.window, |_, window, _| {
            tick(window, driver, viewport, generation)
        })
        .context("starting the scroll driver")?;
        Ok(rx)
    }

    /// Jumps to file `file_idx` (its header at the top).
    pub fn jump_to_file(&self, cx: &mut AsyncApp, file_idx: u32) {
        self.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(file_idx), cx));
    }
}

/// One scroll step on the next display frame, before it is drawn.
fn tick(
    window: &Window,
    driver: Rc<RefCell<Driver>>,
    viewport: Entity<DiffViewport>,
    generation: u64,
) {
    window.on_next_frame(move |window, cx| {
        if driver.borrow().generation != generation {
            return;
        }
        let (top, max) = {
            let doc = viewport.read(cx).document();
            (doc.scroll_top(), doc.max_scroll())
        };
        let step = driver
            .borrow_mut()
            .scroller
            .as_mut()
            .and_then(|s| s.step(Instant::now(), top, max));
        match step {
            Some(dy) => {
                viewport.update(cx, |v, cx| v.scroll_by(dy, cx));
                tick(window, driver, viewport, generation);
            }
            None => {
                let mut d = driver.borrow_mut();
                let last = d.scroller.take().and_then(|s| s.last_step());
                if let Some(done) = d.done.take() {
                    let _ = done.send(last);
                }
            }
        }
    });
}

/// Sleeps on GPUI's timer.
pub async fn sleep(cx: &AsyncApp, d: Duration) {
    cx.background_executor().timer(d).await;
}

/// Sleeps until `t` (returns at once when it has passed).
pub async fn sleep_until(cx: &AsyncApp, t: Instant) {
    let left = t.saturating_duration_since(Instant::now());
    if !left.is_zero() {
        sleep(cx, left).await;
    }
}

/// `rx`'s value, or an error after `timeout`.
pub async fn within<T>(
    cx: &AsyncApp,
    timeout: Duration,
    rx: oneshot::Receiver<T>,
    what: &str,
) -> anyhow::Result<T> {
    let timer = cx.background_executor().timer(timeout);
    match select(rx.fuse(), timer).await {
        Either::Left((Ok(v), _)) => Ok(v),
        Either::Left((Err(_), _)) => anyhow::bail!("{what}: the driver went away"),
        Either::Right(_) => {
            anyhow::bail!("{what}: no frames for {timeout:?} (is the window hidden?)")
        }
    }
}
