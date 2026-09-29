//! Polygloss, the GPUI app. The modules live in this library so the `app`
//! and `e2e` integration test binaries can reach them; `main.rs` is only the
//! entry point.
//!
//! - [`provider`]: [`CoreDiffProvider`], core's opened diffs behind the
//!   viewport's `DiffProvider` trait (kept for M3).
//! - [`gate_shell`]: the M2 gate window (`Polygloss --gate …`); T3.1 replaces
//!   it with the app shell.

pub mod gate_shell;
pub mod provider;

pub use provider::CoreDiffProvider;
