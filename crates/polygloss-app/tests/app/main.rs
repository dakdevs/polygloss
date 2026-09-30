//! The single integration test binary for `polygloss-app` (one module per feature),
//! so the GPUI-linking crate pays for one test link.

#[path = "../support/mod.rs"]
mod support;

mod a11y_keyboard;
mod composer;
mod cursor;
mod e2e_harness;
mod editor;
mod feed;
mod find;
mod home;
mod ipc;
mod iterations;
mod keymap;
mod live;
mod markdown;
mod notify;
mod open_flow;
mod palette;
mod perf;
mod provider;
mod screenshot;
mod settings;
mod shell;
mod submit;
mod theme;
mod threads;
mod tree;
mod urls;
mod version;
mod view_state;
mod viewed;
