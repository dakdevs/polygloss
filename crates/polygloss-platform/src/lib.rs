//! Polygloss platform glue: app launch, Dock badge, Sparkle, editor detection and
//! Install CLI. Links no GPUI, so the slim CLI can use the launcher.
//!
//! `unsafe` is denied crate-wide. Only modules behind the `appkit` feature (objc2
//! FFI) may opt out with a module-level `#![allow(unsafe_code)]`.
#![deny(unsafe_code)]

#[cfg(feature = "appkit")]
pub mod bundle;
#[cfg(feature = "appkit")]
pub mod dock;
pub mod editor;
pub mod launch;
