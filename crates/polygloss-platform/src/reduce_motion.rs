//! macOS Reduce Motion, read and followed live (design §11.16, ADR-0030).
//!
//! AppKit posts `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`
//! on the workspace's notification centre whenever an Accessibility display
//! option changes, Reduce Motion included. [`observe`] registers an observer
//! object of our own class (selector-based, so no `block2`) for it, until
//! the returned [`Observer`] drops.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::thread::{self, ThreadId};

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter};

/// System Settings → Accessibility → Display → Reduce motion.
pub fn system_reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// What the observer object holds.
struct Target {
    /// The thread that observed: the callback is not `Send`, so a post on
    /// any other thread runs nothing (AppKit posts on the main thread).
    thread: ThreadId,
    /// Taken on [`Observer`]'s drop, so the callback is dropped on its own
    /// thread even if a post elsewhere keeps the object alive a moment.
    on_change: RefCell<Option<Box<dyn Fn()>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = Target]
    struct ReduceMotionTarget;

    impl ReduceMotionTarget {
        #[unsafe(method(displayOptionsDidChange:))]
        fn display_options_did_change(&self, _notification: &NSNotification) {
            let target = self.ivars();
            if target.thread != thread::current().id() {
                return;
            }
            if let Some(on_change) = &*target.on_change.borrow() {
                on_change();
            }
        }
    }
);

/// Observes Reduce Motion changes until dropped (on the thread that
/// observed: it is not `Send`).
pub struct Observer {
    center: Retained<NSNotificationCenter>,
    target: Retained<ReduceMotionTarget>,
}

/// Calls `on_change` each time an Accessibility display option changes
/// (read the new value with [`system_reduce_motion`]), on this thread, until
/// the [`Observer`] drops.
pub fn observe(on_change: Box<dyn Fn() + 'static>) -> Observer {
    let target = ReduceMotionTarget::alloc().set_ivars(Target {
        thread: thread::current().id(),
        on_change: RefCell::new(Some(on_change)),
    });
    // SAFETY: NSObject's designated initializer.
    let target: Retained<ReduceMotionTarget> = unsafe { msg_send![super(target), init] };
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: `displayOptionsDidChange:` is defined above and takes the
    // notification; the name is AppKit's own constant; no sender filter.
    unsafe {
        center.addObserver_selector_name_object(
            &target,
            sel!(displayOptionsDidChange:),
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
        );
    }
    Observer { center, target }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: `target` is the object `observe` registered.
        unsafe { self.center.removeObserver(&self.target) };
        self.target.ivars().on_change.take();
    }
}
