//! The stable CLI path (design §13.2, T4.3): `<data_dir>/bin/polygloss` is a
//! symlink to the running bundle's `Contents/MacOS/polygloss-cli`, refreshed
//! by the app at every launch, so the Claude Code plugin's shim (and anything
//! else) finds the CLI of the installed app wherever the app lives. T5.4 adds
//! the in-app Install CLI (`/usr/local/bin/polygloss`) here.
//!
//! The link is replaced atomically: a new symlink is created under a temporary
//! name in the same directory and renamed over the old entry, so a concurrent
//! reader sees the old link or the new one, never neither.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, symlink};
use std::path::{Path, PathBuf};

/// The link's name in the bin dir.
pub const CLI_LINK_NAME: &str = "polygloss";

/// The CLI executable's name inside the bundle.
pub const CLI_BIN_NAME: &str = "polygloss-cli";

/// What [`refresh_stable_symlink`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkRefresh {
    /// There was no entry; the link was created.
    Created,
    /// A link to another target (possibly dangling) or a file was replaced.
    Replaced,
    /// The link already pointed at the target.
    Unchanged,
}

/// Points `<bin_dir>/polygloss` at `target`, creating `bin_dir` (`0700`, like
/// the data dir) if needed. A symlink elsewhere (or dangling) or a regular file
/// at that path is replaced; a directory there is an error and is left alone.
pub fn refresh_stable_symlink(bin_dir: &Path, target: &Path) -> io::Result<SymlinkRefresh> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(bin_dir)?;
    let link = bin_dir.join(CLI_LINK_NAME);
    let outcome = match fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::read_link(&link)? == target {
                return Ok(SymlinkRefresh::Unchanged);
            }
            SymlinkRefresh::Replaced
        }
        Ok(meta) if meta.is_dir() => {
            return Err(io::Error::other(format!(
                "{} is a directory; not replacing it with the CLI link",
                link.display()
            )));
        }
        Ok(_) => SymlinkRefresh::Replaced,
        Err(e) if e.kind() == io::ErrorKind::NotFound => SymlinkRefresh::Created,
        Err(e) => return Err(e),
    };
    let tmp = bin_dir.join(format!(".{CLI_LINK_NAME}.tmp-{}", std::process::id()));
    // A leftover from a crashed refresh by a process with our pid.
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    symlink(target, &tmp)?;
    if let Err(e) = fs::rename(&tmp, &link) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(outcome)
}

/// The `polygloss-cli` next to `app_exe` when `app_exe` runs from an app
/// bundle (`<name>.app/Contents/MacOS/<exe>`) and the CLI exists there.
/// `None` for unbundled dev builds (`target/debug/Polygloss`), so a
/// `cargo run` never repoints the installed app's link.
pub fn bundled_cli(app_exe: &Path) -> Option<PathBuf> {
    let macos = app_exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension().is_some_and(|e| e == "app");
    let cli = macos.join(CLI_BIN_NAME);
    (is_bundle && cli.is_file()).then_some(cli)
}

/// What the app runs at launch: refreshes the link to [`bundled_cli`] of
/// `app_exe`. `Ok(None)` (nothing touched) for an unbundled build.
pub fn refresh_for_exe(bin_dir: &Path, app_exe: &Path) -> io::Result<Option<SymlinkRefresh>> {
    match bundled_cli(app_exe) {
        Some(cli) => refresh_stable_symlink(bin_dir, &cli).map(Some),
        None => Ok(None),
    }
}
