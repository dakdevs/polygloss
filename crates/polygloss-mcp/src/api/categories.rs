//! File categories for agents (design §11.15, ADR-0028): `open_diff`, the
//! diff resource and the CLI reports classify files with the user's
//! `settings.json`, read on every call, so an edit applies to the next call.
//!
//! - Lenient: an invalid `categories` section counts as the defaults, with
//!   one warning on stderr per process.
//! - Agents see `settings.json` only, never the app's per-tab palette
//!   toggles, so their verdicts can differ from what the human sees.

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use polygloss_core::categories::Categorizer;
use polygloss_core::paths::DataPaths;
use polygloss_core::settings::categories_config;

use crate::errors::ApiError;

/// The categorizer of `<config_dir>/settings.json` as it is now.
pub fn categorizer(paths: &DataPaths) -> Result<Categorizer, ApiError> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let read = categories_config(paths);
    if let Some(warning) = &read.warning
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        // Not `eprintln!`, which panics when stderr is closed.
        let _ = writeln!(
            std::io::stderr(),
            "polygloss: {warning}; using the default categories"
        );
    }
    // The lenient read already compiled this config (or fell back to the
    // defaults), so this cannot fail in practice.
    Categorizer::new(&read.config, &read.legacy_generated)
        .map_err(|e| ApiError::internal(format!("categories: {e}")))
}
