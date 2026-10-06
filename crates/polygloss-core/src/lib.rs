//! Polygloss core: git layer, ids, snapshots, store, events and the review
//! domain. Shared by the app and the CLI; never links GPUI, tokio, rmcp or lumis.
//
// `deny`, not `forbid`: Rust 2024 makes `std::env::set_var` unsafe, and only the
// test-support `testing::Sandbox::isolate()` (one process per test under nextest)
// may opt back in, with `#![allow(unsafe_code)]` inside `testing.rs`. Library code
// stays free of `unsafe`.
#![deny(unsafe_code)]

pub mod categories;
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

/// The Polygloss version shared by every binary: `POLYGLOSS_VERSION` at build
/// time when set (a release's CalVer, e.g. `20261005.1`, ADR-0019), else the
/// crate version (`CARGO_PKG_VERSION`, semver; local builds).
pub const VERSION: &str = version(option_env!("POLYGLOSS_VERSION"), env!("CARGO_PKG_VERSION"));

/// `injected` unless it is unset or empty, else `crate_version`.
const fn version(injected: Option<&'static str>, crate_version: &'static str) -> &'static str {
    match injected {
        Some(v) if !v.is_empty() => v,
        _ => crate_version,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_injected_version_replaces_the_crate_version() {
        assert_eq!(super::version(Some("20261005.1"), "0.1.0"), "20261005.1");
        assert_eq!(super::version(None, "0.1.0"), "0.1.0");
        assert_eq!(super::version(Some(""), "0.1.0"), "0.1.0");
    }

    #[test]
    fn crate_version_is_semver() {
        let version = env!("CARGO_PKG_VERSION");
        let core = version.split(['-', '+']).next().unwrap_or_default();
        let parts: Vec<&str> = core.split('.').collect();
        assert_eq!(parts.len(), 3, "{version:?} is not MAJOR.MINOR.PATCH");
        for part in parts {
            assert!(
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
                "{version:?} has a non-numeric component {part:?}"
            );
            assert!(
                part == "0" || !part.starts_with('0'),
                "{version:?} has a leading zero"
            );
        }
    }
}
