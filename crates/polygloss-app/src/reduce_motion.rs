//! macOS Reduce Motion followed live (design §11.16, ADR-0030): when the
//! setting changes, the platform's observer
//! ([`polygloss_platform::reduce_motion::observe`]) reaches the foreground
//! executor, which runs [`motion::follow_system_reduce_motion`] for every
//! window. That runs the [`motion::ReduceMotionSource`] (gpui-base's
//! `apply_system_reduce_motion`, the only writer of `App::reduce_motion`),
//! so app and kit motion follow without an activation; the activation
//! re-read (`motion::init`) stays as a fallback.

use std::any::Any;

use gpui_kit::{App, AsyncApp, Global};

use crate::motion;

/// Starts observing Reduce Motion changes, calling its argument on each,
/// until the returned guard drops (a GPUI global). By default
/// `polygloss_platform`'s observer; tests install a fake that fires on
/// demand.
#[derive(Clone, Copy)]
pub struct ObserverSource(pub fn(OnChange) -> Box<dyn Any>);

/// What an observer calls on each change.
pub type OnChange = Box<dyn Fn() + 'static>;

impl Default for ObserverSource {
    fn default() -> Self {
        ObserverSource(|on_change| Box::new(polygloss_platform::reduce_motion::observe(on_change)))
    }
}

impl Global for ObserverSource {}

/// The observer, kept for the app's life.
struct Observing {
    _observer: Box<dyn Any>,
}

impl Global for Observing {}

/// Observes Reduce Motion for the app's life.
pub fn init(cx: &mut App) {
    let ObserverSource(observe) = cx
        .try_global::<ObserverSource>()
        .copied()
        .unwrap_or_default();
    let app = cx.to_async();
    let observer = observe(Box::new(move || {
        app.spawn(async |cx: &mut AsyncApp| cx.update(follow))
            .detach();
    }));
    cx.set_global(Observing {
        _observer: observer,
    });
}

/// Reads the setting again for every window (the app's are its main
/// windows); a change of the flag redraws them all.
fn follow(cx: &mut App) {
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, cx| {
            motion::follow_system_reduce_motion(window, cx)
        });
    }
}
