//! Polygloss core: git layer, ids, snapshots, store, events and the review
//! domain. Shared by the app and the CLI; never links GPUI, tokio, rmcp or lumis.
//
// `deny`, not `forbid`: Rust 2024 makes `std::env::set_var` unsafe, and only the
// test-support `testing::Sandbox::isolate()` (one process per test under nextest)
// may opt back in, with `#![allow(unsafe_code)]` inside `testing.rs`. Library code
// stays free of `unsafe`.
#![deny(unsafe_code)]

pub mod git;
pub mod ids;
pub mod ipc;
pub mod objects;
pub mod paths;
pub mod process;
pub mod review;
pub mod settings;
pub mod store;
pub mod urls;

#[cfg(feature = "test-support")]
pub mod testing;

pub use ids::{DiffId, DiffIdPrefix, IdError, diff_id, new_uuid};
pub use polygloss_diff::{ObjectFormat, Oid};

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
