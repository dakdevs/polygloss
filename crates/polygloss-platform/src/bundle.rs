//! Whether the process runs from an app bundle (design §17, §21).
//!
//! GPUI's `App::show_system_notification` is built on
//! `UNUserNotificationCenter`, which aborts the process outside an app bundle
//! (library-choices §12), so the app only posts notifications when
//! [`is_bundled`]. The test is the one the framework itself relies on: the
//! main bundle has a bundle identifier (a `cargo run` or test binary has
//! none).

/// The main bundle's `CFBundleIdentifier` (`dev.dak.polygloss` for the
/// packaged app); `None` outside an app bundle or off macOS.
pub fn bundle_identifier() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::NSBundle;
        NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|id| id.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Whether the process runs from an app bundle (has a bundle identifier).
pub fn is_bundled() -> bool {
    bundle_identifier().is_some()
}
