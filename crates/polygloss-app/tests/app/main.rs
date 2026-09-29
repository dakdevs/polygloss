//! The single integration test binary for `polygloss-app` (one module per feature),
//! so the GPUI-linking crate pays for one test link.

#[path = "../support/mod.rs"]
mod support;

mod e2e_harness;
mod gate_shell;
mod provider;
mod screenshot;
mod version;
