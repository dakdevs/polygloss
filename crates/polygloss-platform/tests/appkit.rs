//! Bundle detection and the Dock badge (T3.17, design §17, library-choices
//! §12). A test binary is not an app bundle, and libtest runs tests off the
//! main thread, where AppKit must not be touched: both calls are safe
//! no-ops there, so nothing here changes the real Dock.

use polygloss_platform::{bundle, dock};

#[test]
fn a_test_binary_is_not_bundled() {
    assert!(!bundle::is_bundled());
    assert_eq!(bundle::bundle_identifier(), None);
}

#[test]
fn set_badge_off_the_main_thread_does_nothing() {
    assert_ne!(std::thread::current().name(), Some("main"));
    assert!(
        !dock::set_badge(Some("3")),
        "not applied off the main thread"
    );
    assert!(!dock::set_badge(None));
}

#[test]
fn badge_labels_show_the_count_and_hide_zero() {
    assert_eq!(dock::badge_label(0), None);
    assert_eq!(dock::badge_label(1).as_deref(), Some("1"));
    assert_eq!(dock::badge_label(42).as_deref(), Some("42"));
}
