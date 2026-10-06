//! Live Reduce Motion (T7.16, ADR-0030, design §11.16): a change of the
//! setting reaches every main window without an activation, and the
//! activation re-read stays. A fake `ObserverSource` keeps the callback for
//! the test to fire (the system's notification); a fake `ReduceMotionSource`
//! is the system's reading (gpui-base's never reads under the test
//! scheduler).

use std::any::Any;
use std::cell::RefCell;

use gpui_kit::base::{Root, RootPlugin};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, IntoElement, Render, TestAppContext,
    VisualTestContext, Window, div,
};
use polygloss_app::motion::ReduceMotionSource;
use polygloss_app::reduce_motion::ObserverSource;

use crate::shell::{draw, start};
use crate::support::Sandbox;

thread_local! {
    /// The callback `reduce_motion::init` gave the fake observer.
    static ON_CHANGE: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
    /// Every window drawn, with Reduce Motion as the draw read it.
    static DRAWN: RefCell<Vec<(AnyWindowHandle, bool)>> = const { RefCell::new(Vec::new()) };
}

/// The fake observer: keeps the callback for [`fire`].
fn fake_observe(on_change: Box<dyn Fn()>) -> Box<dyn Any> {
    ON_CHANGE.with(|slot| *slot.borrow_mut() = Some(on_change));
    Box::new(())
}

/// What the system's notification does: runs the observer's callback.
fn fire() {
    ON_CHANGE.with(|slot| (slot.borrow().as_ref().expect("init installed the observer"))());
}

/// Records each draw of a window: gpui-kit's root runs every plugin's
/// `prepare` each time it renders, and a window's root renders on every
/// draw.
struct DrawProbe;

impl Render for DrawProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl RootPlugin for DrawProbe {
    fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let drawn = (window.window_handle(), cx.reduce_motion());
        DRAWN.with(|d| d.borrow_mut().push(drawn));
    }
}

fn reduce_motion(cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| cx.reduce_motion())
}

#[gpui_kit::test]
fn reduce_motion_change_is_followed_without_activation(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    cx.update(|cx| {
        cx.set_global(ObserverSource(fake_observe));
        Root::register_plugin(cx, |_, _| DrawProbe);
    });
    let shell = start(cx);
    let main = shell.cx.update(|window, _| window.window_handle());
    // The app has one main window (a second one redraws forever in tests),
    // so a kit-rooted probe window stands in for any other window.
    let other: AnyWindowHandle = shell
        .cx
        .cx
        .add_window(|window, cx| Root::new(cx.new(|_| DrawProbe), window, cx))
        .into();
    draw(shell.cx);

    // The system turns Reduce Motion on while the app runs.
    shell
        .cx
        .update(|_, cx| cx.set_global(ReduceMotionSource(|cx| cx.set_reduce_motion(true))));
    DRAWN.with(|d| d.borrow_mut().clear());
    shell.cx.run_until_parked();
    assert!(!reduce_motion(shell.cx), "not before the notification");
    assert_eq!(
        DRAWN.with(|d| d.borrow().len()),
        0,
        "nothing drew meanwhile"
    );

    fire();
    shell.cx.run_until_parked();
    assert!(reduce_motion(shell.cx), "followed without an activation");
    let drawn = DRAWN.with(|d| d.borrow().clone());
    for window in [main, other] {
        assert!(
            drawn.contains(&(window, true)),
            "{window:?} drew a frame under Reduce Motion: {drawn:?}"
        );
    }
}

#[gpui_kit::test]
fn the_activation_re_read_still_follows_the_system(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    cx.update(|cx| cx.set_global(ObserverSource(fake_observe)));
    let shell = start(cx);
    assert!(
        ON_CHANGE.with(|slot| slot.borrow().is_some()),
        "the observer is installed (and never fires here)"
    );
    let activate = |cx: &mut VisualTestContext| {
        cx.deactivate_window();
        cx.update(|window, _| window.activate_window());
        draw(cx);
    };

    shell
        .cx
        .update(|_, cx| cx.set_global(ReduceMotionSource(|cx| cx.set_reduce_motion(true))));
    activate(shell.cx);
    assert!(reduce_motion(shell.cx), "read on activation");

    shell
        .cx
        .update(|_, cx| cx.set_global(ReduceMotionSource(|cx| cx.set_reduce_motion(false))));
    activate(shell.cx);
    assert!(!reduce_motion(shell.cx), "and off again");
}
