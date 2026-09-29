//! The Dock badge (design §17): the number of reviews awaiting you, set
//! through `NSApplication.dockTile.badgeLabel`, since GPUI has no Dock-tile
//! API (library-choices §12).

/// The badge text for `count` reviews awaiting you: none for zero.
pub fn badge_label(count: u32) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// Sets (or with `None` clears) the app's Dock badge. AppKit may only be
/// used on the main thread: anywhere else (and off macOS) this does
/// nothing. Returns whether the badge was set.
pub fn set_badge(label: Option<&str>) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        use objc2_foundation::NSString;

        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let label = label.map(NSString::from_str);
        NSApplication::sharedApplication(mtm)
            .dockTile()
            .setBadgeLabel(label.as_deref());
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = label;
        false
    }
}
