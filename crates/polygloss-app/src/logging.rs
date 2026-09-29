//! The app's log: a daily rolling file in `DataPaths.logs_dir`
//! (`~/Library/Logs/polygloss/polygloss.<date>.log`, design §13.2), a week
//! of files kept. GPUI's `log` records are bridged in. `POLYGLOSS_LOG` (an
//! `EnvFilter` directive) overrides the default level.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;

/// What is logged unless `POLYGLOSS_LOG` says otherwise.
pub const DEFAULT_FILTER: &str =
    "warn,polygloss_app=info,polygloss_core=info,polygloss_viewport=info";

/// Log files kept.
const KEEP_FILES: usize = 7;

/// Starts logging to `logs_dir` (created if missing). Keep the guard for
/// the life of the process: dropping it flushes and stops the writer.
/// Returns `None`, logging nowhere, when the directory cannot be used or a
/// logger is already installed.
pub fn init(logs_dir: &Path) -> Option<WorkerGuard> {
    std::fs::create_dir_all(logs_dir).ok()?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("polygloss")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(logs_dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter =
        EnvFilter::try_from_env("POLYGLOSS_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init()
        .ok()?;
    Some(guard)
}
