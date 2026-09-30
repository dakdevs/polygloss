//! Launching the app for the CLI and the MCP server (T4.2, design §13.4).
//!
//! [`ensure_app`] probes the app socket; when nothing listens there it asks a
//! [`Launcher`] to start the app, then polls the socket for up to
//! [`LAUNCH_TIMEOUT`] (10 s, provisional). The caller sends its socket op
//! (`open`, `focus`, …) afterwards; the URL given here only reaches an app that
//! this call launches.
//!
//! [`SystemLauncher::Open`] runs `open -g -b dev.dak.polygloss [url]`
//! (LaunchServices: one instance, no focus stolen with `-g`; `activate` drops
//! the `-g`). Under `POLYGLOSS_TEST=1`, `$POLYGLOSS_APP_BIN` names an unbundled
//! app binary to spawn instead (OQ-P4), detached, with the URL as its argument.
//! Either one without the other launches nothing ([`SystemLauncher::Refused`])
//! rather than the installed bundle on the real data dir.

use std::ffi::OsString;
use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use polygloss_core::paths::{DataPaths, socket_path_fits};
use sha2::{Digest as _, Sha256};

/// The app's bundle id.
pub const BUNDLE_ID: &str = "dev.dak.polygloss";

/// How long [`ensure_app`] waits for a launched app's socket (**Provisional**).
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the socket is probed while waiting.
const POLL: Duration = Duration::from_millis(25);

const OPEN: &str = "/usr/bin/open";

/// What [`ensure_app`] found or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// The app was already listening; nothing was launched.
    AlreadyRunning,
    /// The app was launched and its socket is up.
    Launched,
    /// The launch failed or the socket did not come up in time (the message
    /// says which).
    Unavailable(String),
}

/// Starts the app. Returns once the launch was requested; [`ensure_app`] waits
/// for the socket.
pub trait Launcher {
    fn launch(&self, url: Option<&str>, activate: bool) -> io::Result<()>;
}

/// Makes sure the app runs: [`LaunchOutcome::AlreadyRunning`] when its socket
/// answers, else launches it through `launcher` (with `url`, in the
/// background unless `activate`) and waits up to [`LAUNCH_TIMEOUT`] for the
/// socket.
pub fn ensure_app(
    paths: &DataPaths,
    url: Option<&str>,
    activate: bool,
    launcher: &dyn Launcher,
) -> LaunchOutcome {
    ensure_app_within(paths, url, activate, launcher, LAUNCH_TIMEOUT)
}

/// [`ensure_app`] waiting at most `timeout` for the socket.
pub fn ensure_app_within(
    paths: &DataPaths,
    url: Option<&str>,
    activate: bool,
    launcher: &dyn Launcher,
    timeout: Duration,
) -> LaunchOutcome {
    if app_is_running(paths) {
        return LaunchOutcome::AlreadyRunning;
    }
    if let Err(e) = launcher.launch(url, activate) {
        return LaunchOutcome::Unavailable(format!("could not launch Polygloss: {e}"));
    }
    let deadline = Instant::now() + timeout;
    loop {
        if app_is_running(paths) {
            return LaunchOutcome::Launched;
        }
        let now = Instant::now();
        if now >= deadline {
            return LaunchOutcome::Unavailable(format!(
                "Polygloss did not start within {:.1} s",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(POLL.min(deadline - now));
    }
}

/// True when something accepts connections on the app socket. A socket file
/// left behind by a crashed app refuses them.
pub fn app_is_running(paths: &DataPaths) -> bool {
    UnixStream::connect(app_socket_path(paths)).is_ok()
}

/// Where the app listens: `paths.socket` when it fits in `sun_path`, else a
/// per-user fallback (design §13.2; see [`app_socket_path_with`]).
pub fn app_socket_path(paths: &DataPaths) -> PathBuf {
    app_socket_path_with(
        paths,
        std::env::var_os("TMPDIR").map(PathBuf::from).as_deref(),
    )
}

/// [`app_socket_path`] with an explicit `$TMPDIR`: `paths.socket`, else
/// `<tmpdir>/polygloss-<euid>/polygloss-<16 hex of sha256(data_dir)>.sock`
/// (`/tmp` when `tmpdir` is unset, relative or too long), so two data dirs
/// that both need the fallback never share a socket. The same rule as T4.1's
/// `polygloss_core::ipc::socket_path`, which the server binds (merge note:
/// delegate to it once both are in).
pub fn app_socket_path_with(paths: &DataPaths, tmpdir: Option<&Path>) -> PathBuf {
    if socket_path_fits(&paths.socket) {
        return paths.socket.clone();
    }
    let hash = Sha256::digest(paths.data_dir.as_os_str().as_encoded_bytes());
    let name = format!("polygloss-{}.sock", &hex::encode(hash)[..16]);
    let dir = format!("polygloss-{}", current_uid());
    let in_tmpdir = tmpdir
        .filter(|t| t.is_absolute())
        .map(|t| t.join(&dir).join(&name))
        .filter(|p| socket_path_fits(p));
    in_tmpdir.unwrap_or_else(|| Path::new("/tmp").join(dir).join(name))
}

/// This process's effective uid.
#[allow(unsafe_code)]
fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, cannot fail and touches no memory.
    unsafe { libc::geteuid() }
}

/// Starts the real app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemLauncher {
    /// `open [-g] -b dev.dak.polygloss [url]`: the installed bundle.
    Open,
    /// An unbundled app binary (`$POLYGLOSS_APP_BIN` under `POLYGLOSS_TEST=1`).
    AppBin(PathBuf),
    /// Launches nothing and fails with this reason: only one of
    /// `POLYGLOSS_TEST=1` and `$POLYGLOSS_APP_BIN` is set. That is a
    /// misconfigured test or gate run, and falling back to the installed
    /// bundle would start the real app on the real data dir (LaunchServices
    /// ignores the caller's environment, so a sandbox's
    /// `POLYGLOSS_DATA_DIR` never reaches it).
    Refused(String),
}

impl SystemLauncher {
    /// From the process environment (see [`SystemLauncher::from_env_with`]).
    pub fn from_env() -> SystemLauncher {
        Self::from_env_with(|k| std::env::var_os(k))
    }

    /// [`SystemLauncher::AppBin`] when `POLYGLOSS_TEST=1` and
    /// `POLYGLOSS_APP_BIN` (not empty) are both set,
    /// [`SystemLauncher::Refused`] when only one of them is, else
    /// [`SystemLauncher::Open`].
    pub fn from_env_with(env: impl Fn(&str) -> Option<OsString>) -> SystemLauncher {
        let test = env("POLYGLOSS_TEST").is_some_and(|v| v == "1");
        match (env("POLYGLOSS_APP_BIN").filter(|v| !v.is_empty()), test) {
            (Some(bin), true) => SystemLauncher::AppBin(bin.into()),
            (Some(_), false) => SystemLauncher::Refused(
                "POLYGLOSS_APP_BIN is set but POLYGLOSS_TEST=1 is not; \
                 not launching the installed app instead"
                    .to_owned(),
            ),
            (None, true) => SystemLauncher::Refused(
                "POLYGLOSS_TEST=1 is set but POLYGLOSS_APP_BIN is not; \
                 not launching the installed app on the real data dir"
                    .to_owned(),
            ),
            (None, false) => SystemLauncher::Open,
        }
    }

    /// The command line that starts the app (empty for
    /// [`SystemLauncher::Refused`]).
    pub fn argv(&self, url: Option<&str>, activate: bool) -> Vec<OsString> {
        let mut argv: Vec<OsString> = match self {
            SystemLauncher::Open => {
                let mut argv = vec![OsString::from(OPEN)];
                if !activate {
                    argv.push("-g".into());
                }
                argv.extend(["-b".into(), BUNDLE_ID.into()]);
                argv
            }
            SystemLauncher::AppBin(bin) => vec![bin.clone().into_os_string()],
            SystemLauncher::Refused(_) => return Vec::new(),
        };
        argv.extend(url.map(OsString::from));
        argv
    }
}

impl Launcher for SystemLauncher {
    fn launch(&self, url: Option<&str>, activate: bool) -> io::Result<()> {
        let argv = self.argv(url, activate);
        match self {
            SystemLauncher::Open => run_open(&argv),
            SystemLauncher::AppBin(_) => spawn_detached(&argv),
            SystemLauncher::Refused(reason) => Err(io::Error::other(reason.clone())),
        }
    }
}

/// Runs `open`, which returns once LaunchServices took the request.
fn run_open(argv: &[OsString]) -> io::Result<()> {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(io::Error::other(format!(
        "`open -b {BUNDLE_ID}` failed ({}): {}",
        out.status,
        stderr.trim()
    )))
}

/// Starts the app binary in its own process group with stdio closed, and
/// reaps it on a thread, so the caller never waits for it.
fn spawn_detached(argv: &[OsString]) -> io::Result<()> {
    let mut child = Command::new(Path::new(&argv[0]))
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    std::thread::Builder::new()
        .name("polygloss-app-wait".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}
