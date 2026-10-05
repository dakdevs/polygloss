//! Polygloss, the GPUI app. The modules live in this library so the `app`
//! and `e2e` integration test binaries can reach them; `main.rs` is only the
//! entry point ([`startup::main`]).
//!
//! The shell (T3.1): [`startup`] (arguments, logging, the store, the GPUI
//! app), [`app_state`], [`settings`], [`window`] (the one main window and
//! the menu bar), [`chrome`] (window options, the sidebar and the top rows
//! every page renders, T6.3), [`assets`] (the icons, T6.3), [`motion`]
//! (the two entrances and Reduce Motion, T6.8), [`tabs`],
//! [`review_tab`] (toolbar, banner strip, panes),
//! [`provider`] ([`CoreDiffProvider`]), [`logging`], [`perf`] (test-only
//! `--perf-scenario`), [`dump`] (the hidden `--dump-keymap`/`--dump-settings`)
//! and [`features`], which wires every feature module in:
//! each exposes `init(cx)`, review-tab features `attach`, and toolbar and
//! pane contributors their render functions (plan M3 "App module map").

pub mod app_state;
pub mod assets;
pub mod categories;
pub mod chrome;
pub mod composer;
pub mod cursor;
pub mod dump;
pub mod editor;
pub mod features;
pub mod feed;
pub mod find;
pub mod home;
pub mod install_cli;
pub mod ipc;
pub mod iterations;
pub mod keyboard;
pub mod keymap;
pub mod live;
pub mod logging;
pub mod markdown;
pub mod motion;
pub mod notify;
pub mod open_flow;
pub mod palette;
pub mod perf;
pub mod provider;
pub mod review_tab;
pub mod settings;
pub mod startup;
pub mod submit;
pub mod tabs;
pub mod theme;
pub mod threads;
pub mod tree;
pub mod updates;
pub mod urls;
pub mod view_state;
pub mod viewed;
pub mod window;

pub use provider::CoreDiffProvider;
