//! Polygloss platform glue: app launch, Dock badge, Sparkle, Reduce Motion, editor
//! detection and Install CLI. Links no GPUI, so the slim CLI can use the launcher.
//!
//! `unsafe` is denied crate-wide. Only modules behind the `appkit` feature (objc2
//! FFI) may opt out with a module-level `#![allow(unsafe_code)]`, plus the one
//! `getuid` call in `launch` (a narrow `#[allow(unsafe_code)]` on that function).
#![deny(unsafe_code)]

#[cfg(feature = "appkit")]
pub mod bundle;
#[cfg(feature = "appkit")]
pub mod dock;
pub mod editor;
pub mod install;
pub mod launch;
#[cfg(feature = "appkit")]
pub mod reduce_motion;
#[cfg(feature = "appkit")]
pub mod sparkle;
