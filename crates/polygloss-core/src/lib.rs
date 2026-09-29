//! Polygloss core: git layer, ids, snapshots, store, events and the review
//! domain. Shared by the app and the CLI; never links GPUI, tokio, rmcp or lumis.
#![forbid(unsafe_code)]

/// The Polygloss version shared by every binary (`CARGO_PKG_VERSION`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        let core = VERSION.split(['-', '+']).next().unwrap_or_default();
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "{VERSION:?} is not MAJOR.MINOR.PATCH");
        for part in parts {
            assert!(
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
                "{VERSION:?} has a non-numeric component {part:?}"
            );
            assert!(
                part == "0" || !part.starts_with('0'),
                "{VERSION:?} has a leading zero"
            );
        }
    }
}
