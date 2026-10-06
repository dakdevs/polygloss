//! The live Reduce Motion observer (T7.16, ADR-0030, design §11.16). The
//! workspace's notification centre belongs to this process, so a post here
//! reaches only this process's observers: never the system, another app or
//! the setting itself.

// Named `reduce_motion::…` like the app's tests, so one nextest filter,
// `test(/reduce_motion/)`, selects both.
mod reduce_motion {
    use std::cell::Cell;
    use std::rc::Rc;

    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use polygloss_platform::reduce_motion;

    /// Posts what macOS posts when an Accessibility display option changes.
    fn post() {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the name is AppKit's own constant, and the post carries no
        // object.
        unsafe {
            center.postNotificationName_object(
                NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
                None,
            )
        };
    }

    /// An observer counting its calls.
    fn counting() -> (reduce_motion::Observer, Rc<Cell<u32>>) {
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        let observer = reduce_motion::observe(Box::new(move || counter.set(counter.get() + 1)));
        (observer, calls)
    }

    #[test]
    fn a_posted_notification_reaches_the_callback_until_drop() {
        let (observer, calls) = counting();
        assert_eq!(calls.get(), 0, "nothing before a post");
        post();
        assert_eq!(calls.get(), 1, "one post, one call");
        drop(observer);
        post();
        assert_eq!(calls.get(), 1, "removed when dropped");
    }

    #[test]
    fn a_post_on_another_thread_runs_nothing() {
        // The callback is not `Send`: only the thread that observed may run it.
        let (_observer, calls) = counting();
        std::thread::spawn(post).join().unwrap();
        assert_eq!(calls.get(), 0);
        post();
        assert_eq!(calls.get(), 1, "still observing on its own thread");
    }
}
