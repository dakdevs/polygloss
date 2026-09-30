//! The stable CLI path (design §13.2, T4.3): `<data_dir>/bin/polygloss` is a
//! symlink to the running bundle's `Contents/MacOS/polygloss-cli`, refreshed
//! by the app at every launch, so the Claude Code plugin's shim (and anything
//! else) finds the CLI of the installed app wherever the app lives.
//!
//! Install CLI (design §21, ADR-0019, T5.4): [`install_cli`] points
//! [`INSTALL_LINK`] (`/usr/local/bin/polygloss`) at the bundle's CLI for DMG
//! installs (Homebrew's cask links its own). Written from scratch (Zed's is
//! GPL). When the user may not write there it asks for an administrator
//! password through `osascript`, passing every path as a separate argument
//! (the script shell-quotes them with `quoted form of`), never spliced into
//! script or shell text. A real file or directory already at the link path is
//! never replaced.
//!
//! Links are replaced atomically: a new symlink is created under a temporary
//! name in the same directory and renamed over the old entry, so a concurrent
//! reader sees the old link or the new one, never neither.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Where Install CLI puts the `polygloss` command.
pub const INSTALL_LINK: &str = "/usr/local/bin/polygloss";

/// Why Install CLI failed.
#[derive(Debug)]
pub enum InstallError {
    /// The CLI to link to is not an existing file at an absolute path.
    TargetMissing(PathBuf),
    /// A file or directory (not a symlink) is at the link path; it is left
    /// alone.
    Occupied(PathBuf),
    /// The user cancelled the administrator prompt.
    Cancelled,
    /// The administrator step failed.
    Admin(String),
    /// The administrator step reported success, but the link is not there.
    NotLinked(PathBuf),
    /// Any other I/O failure.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::TargetMissing(p) => {
                write!(f, "the command line tool is missing: {}", p.display())
            }
            InstallError::Occupied(p) => write!(
                f,
                "{} already exists and is not a link; remove it first",
                p.display()
            ),
            InstallError::Cancelled => write!(f, "cancelled"),
            InstallError::Admin(message) => {
                write!(f, "the administrator step failed: {message}")
            }
            InstallError::NotLinked(p) => write!(f, "{} was not linked", p.display()),
            InstallError::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InstallError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// What [`install_cli_at`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliInstall {
    /// The link was created or repointed directly.
    Linked,
    /// The link was made through the administrator prompt.
    LinkedAsAdmin,
    /// The link already pointed at the target.
    Unchanged,
}

/// What an administrator command printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminOutput {
    pub success: bool,
    pub stderr: String,
}

/// Runs the administrator fallback's argv (tests record it instead).
pub trait AdminRunner: Send + Sync {
    /// Runs `argv` (no shell) and waits for it.
    fn run(&self, argv: &[OsString]) -> io::Result<AdminOutput>;
}

/// Runs the argv for real: `osascript` shows the system password prompt.
pub struct OsascriptRunner;

impl AdminRunner for OsascriptRunner {
    fn run(&self, argv: &[OsString]) -> io::Result<AdminOutput> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty argv"))?;
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()?;
        Ok(AdminOutput {
            success: out.status.success(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// The AppleScript of the administrator fallback, one `-e` line each. It
/// reads the link's dir, the target and the link from its arguments.
const ADMIN_SCRIPT: [&str; 6] = [
    "on run argv",
    "set binDir to item 1 of argv",
    "set targetPath to item 2 of argv",
    "set linkPath to item 3 of argv",
    "do shell script \"/bin/mkdir -p \" & quoted form of binDir & \" && /bin/ln -sfh \" & quoted form of targetPath & \" \" & quoted form of linkPath with prompt (\"Polygloss wants to install the polygloss command line tool at \" & linkPath & \".\") with administrator privileges",
    "end run",
];

/// The `osascript` argv that makes `link` point at `target` as an
/// administrator: the script, then the link's dir, the target and the link as
/// separate arguments.
pub fn admin_link_argv(link: &Path, target: &Path) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec!["/usr/bin/osascript".into()];
    for line in ADMIN_SCRIPT {
        argv.push("-e".into());
        argv.push(line.into());
    }
    let dir = link.parent().unwrap_or(Path::new("/"));
    argv.extend([dir.into(), target.into(), link.into()]);
    argv
}

/// Install CLI: points [`INSTALL_LINK`] at `target` (the bundle's
/// `polygloss-cli`), asking for an administrator password when needed.
pub fn install_cli(target: &Path) -> Result<(), InstallError> {
    install_cli_at(Path::new(INSTALL_LINK), target, &OsascriptRunner).map(|_| ())
}

/// Points `link` at `target`: creates the link's dir if needed, replaces a
/// symlink (to anything, or dangling) and refuses a file or a directory.
/// When permission is denied, runs [`admin_link_argv`] through `admin` and
/// checks the link it made.
pub fn install_cli_at(
    link: &Path,
    target: &Path,
    admin: &dyn AdminRunner,
) -> Result<CliInstall, InstallError> {
    if !target.is_absolute() || !target.is_file() {
        return Err(InstallError::TargetMissing(target.to_path_buf()));
    }
    let io_err = |path: &Path, source: io::Error| InstallError::Io {
        path: path.to_path_buf(),
        source,
    };
    let (Some(dir), Some(name)) = (link.parent(), link.file_name()) else {
        return Err(io_err(link, io::ErrorKind::InvalidInput.into()));
    };
    if !link.is_absolute() {
        return Err(io_err(
            link,
            io::Error::new(io::ErrorKind::InvalidInput, "not an absolute path"),
        ));
    }
    match fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::read_link(link).map_err(|e| io_err(link, e))? == target {
                return Ok(CliInstall::Unchanged);
            }
        }
        Ok(_) => return Err(InstallError::Occupied(link.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(link, e)),
    }
    match link_directly(dir, name, link, target) {
        Ok(()) => Ok(CliInstall::Linked),
        Err((_, e)) if e.kind() == io::ErrorKind::PermissionDenied => {
            link_as_admin(link, target, admin)
        }
        Err((path, e)) => Err(io_err(&path, e)),
    }
}

/// Creates `dir` and renames a fresh temporary symlink over `link`. The
/// error carries the path it concerns.
fn link_directly(
    dir: &Path,
    name: &std::ffi::OsStr,
    link: &Path,
    target: &Path,
) -> Result<(), (PathBuf, io::Error)> {
    fs::create_dir_all(dir).map_err(|e| (dir.to_path_buf(), e))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".install-{}", std::process::id()));
    let tmp = dir.join(tmp_name);
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err((tmp, e)),
    }
    symlink(target, &tmp).map_err(|e| (tmp.clone(), e))?;
    if let Err(e) = fs::rename(&tmp, link) {
        let _ = fs::remove_file(&tmp);
        return Err((link.to_path_buf(), e));
    }
    Ok(())
}

/// The administrator fallback, then a check that the link is right.
fn link_as_admin(
    link: &Path,
    target: &Path,
    admin: &dyn AdminRunner,
) -> Result<CliInstall, InstallError> {
    let out = admin
        .run(&admin_link_argv(link, target))
        .map_err(|e| InstallError::Io {
            path: PathBuf::from("/usr/bin/osascript"),
            source: e,
        })?;
    if !out.success {
        // AppleScript's "User canceled" error.
        if out.stderr.contains("(-128)") {
            return Err(InstallError::Cancelled);
        }
        return Err(InstallError::Admin(out.stderr.trim().to_owned()));
    }
    match fs::read_link(link) {
        Ok(to) if to == target => Ok(CliInstall::LinkedAsAdmin),
        _ => Err(InstallError::NotLinked(link.to_path_buf())),
    }
}
