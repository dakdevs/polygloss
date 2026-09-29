//! Data, cache, socket, log and config paths; `POLYGLOSS_DATA_DIR` override (T1.10).
//!
//! Layout (design §13.2, macOS):
//!
//! ```text
//! ~/Library/Application Support/polygloss/   data_dir (POLYGLOSS_DATA_DIR overrides)
//!   polygloss.db, polygloss.db.lock, polygloss.sock, app.lock, bin/
//! ~/Library/Caches/polygloss/                cache_dir: scratch/, blobs/
//! ~/Library/Logs/polygloss/                  logs_dir
//! $XDG_CONFIG_HOME|~/.config /polygloss/     config_dir
//! ```
//!
//! Resolution only computes paths; nothing is created here (`Store::open` creates
//! the data dir with mode `0700`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The longest socket path, in bytes, that `bind(2)` accepts on macOS: `sun_path`
/// is 104 bytes including the terminating NUL. Longer paths need the `$TMPDIR`
/// fallback of design §13.2 (implemented by T4.1's `ipc::socket_path`).
pub const SOCKET_PATH_MAX: usize = 103;

/// Environment variable that overrides the data directory (design §7.1).
pub const DATA_DIR_ENV: &str = "POLYGLOSS_DATA_DIR";

/// Every on-disk location Polygloss uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataPaths {
    /// `~/Library/Application Support/polygloss` or `$POLYGLOSS_DATA_DIR`.
    pub data_dir: PathBuf,
    /// `<data_dir>/polygloss.db`.
    pub db: PathBuf,
    /// `<data_dir>/polygloss.db.lock`: held while migrating.
    pub db_lock: PathBuf,
    /// `<data_dir>/polygloss.sock`: the app's IPC socket (see [`Self::socket_path_too_long`]).
    pub socket: PathBuf,
    /// `<data_dir>/app.lock`: dev single-instance lock.
    pub app_lock: PathBuf,
    /// `<data_dir>/bin`: stable `polygloss` CLI symlink.
    pub bin_dir: PathBuf,
    /// `~/Library/Caches/polygloss`.
    pub cache_dir: PathBuf,
    /// `<cache_dir>/scratch`: unpinned snapshot object stores (§5).
    pub scratch_dir: PathBuf,
    /// `<cache_dir>/blobs`: read-only blob copies for open-in-editor.
    pub blobs_dir: PathBuf,
    /// `~/Library/Logs/polygloss`: the app's rolling log.
    pub logs_dir: PathBuf,
    /// `$XDG_CONFIG_HOME/polygloss` or `~/.config/polygloss`.
    pub config_dir: PathBuf,
}

/// Why the paths could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    /// `HOME` is unset or empty.
    #[error("HOME is not set")]
    NoHome,
    /// A variable that must hold an absolute path holds a relative one.
    #[error("{var} must be an absolute path, got {value:?}")]
    NotAbsolute {
        /// The variable name.
        var: &'static str,
        /// Its value.
        value: PathBuf,
    },
}

impl DataPaths {
    /// Resolves the paths from the process environment.
    pub fn resolve() -> Result<DataPaths, PathsError> {
        Self::resolve_with(|k| std::env::var_os(k))
    }

    /// Resolves the paths from `env` (a variable lookup), so callers and tests can
    /// resolve without touching the process environment. Empty values count as unset.
    pub fn resolve_with(env: impl Fn(&str) -> Option<OsString>) -> Result<DataPaths, PathsError> {
        let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);

        let home = var("HOME").ok_or(PathsError::NoHome)?;
        if !home.is_absolute() {
            return Err(PathsError::NotAbsolute {
                var: "HOME",
                value: home,
            });
        }

        let data_dir = match var(DATA_DIR_ENV) {
            Some(dir) if dir.is_absolute() => dir,
            Some(dir) => {
                return Err(PathsError::NotAbsolute {
                    var: DATA_DIR_ENV,
                    value: dir,
                });
            }
            None => platform::data_dir(&home, &var),
        };
        let cache_dir = platform::cache_dir(&home, &var);
        // XDG: a relative value is invalid and must be ignored.
        let config_home = var("XDG_CONFIG_HOME")
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));

        Ok(DataPaths {
            db: data_dir.join("polygloss.db"),
            db_lock: data_dir.join("polygloss.db.lock"),
            socket: data_dir.join("polygloss.sock"),
            app_lock: data_dir.join("app.lock"),
            bin_dir: data_dir.join("bin"),
            scratch_dir: cache_dir.join("scratch"),
            blobs_dir: cache_dir.join("blobs"),
            logs_dir: platform::logs_dir(&home, &var),
            config_dir: config_home.join("polygloss"),
            cache_dir,
            data_dir,
        })
    }

    /// True when [`Self::socket`] is too long to bind (more than
    /// [`SOCKET_PATH_MAX`] bytes), so IPC must use the `$TMPDIR` fallback (T4.1).
    pub fn socket_path_too_long(&self) -> bool {
        !socket_path_fits(&self.socket)
    }
}

/// True when `path` fits in macOS's `sun_path` (at most [`SOCKET_PATH_MAX`] bytes).
pub fn socket_path_fits(path: &Path) -> bool {
    path.as_os_str().len() <= SOCKET_PATH_MAX
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::{Path, PathBuf};

    pub fn data_dir(home: &Path, _var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
        home.join("Library/Application Support/polygloss")
    }

    pub fn cache_dir(home: &Path, _var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
        home.join("Library/Caches/polygloss")
    }

    pub fn logs_dir(home: &Path, _var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
        home.join("Library/Logs/polygloss")
    }
}

// Not a v1 platform (design §22); XDG base directories keep the crate portable.
#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::{Path, PathBuf};

    fn xdg(var: &dyn Fn(&str) -> Option<PathBuf>, key: &str, home: &Path, rel: &str) -> PathBuf {
        var(key)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(rel))
            .join("polygloss")
    }

    pub fn data_dir(home: &Path, var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
        xdg(var, "XDG_DATA_HOME", home, ".local/share")
    }

    pub fn cache_dir(home: &Path, var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
        xdg(var, "XDG_CACHE_HOME", home, ".cache")
    }

    pub fn logs_dir(home: &Path, var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
        xdg(var, "XDG_STATE_HOME", home, ".local/state").join("logs")
    }
}
