//! The single integration test binary for `polygloss-app` (one module per feature),
//! so the GPUI-linking crate pays for one test link.

#[path = "../support/mod.rs"]
mod support;

mod a11y_keyboard;
mod categories;
mod composer;
mod cursor;
mod dump;
mod e2e_harness;
mod editor;
mod feed;
mod find;
mod header_card;
mod home;
mod install_cli;
mod ipc;
mod iterations;
mod keymap;
mod live;
mod markdown;
mod motion;
mod notify;
mod open_flow;
mod palette;
mod perf;
mod provider;
mod reduce_motion;
mod screenshot;
mod settings;
mod shell;
mod submit;
mod theme;
mod threads;
mod toolbar;
mod tree;
mod updates;
mod urls;
mod version;
mod view_state;
mod viewed;
