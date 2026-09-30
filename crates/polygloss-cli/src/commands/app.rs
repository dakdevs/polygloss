//! Showing a review in the app for the human commands (design §13.4, T4.3):
//! make sure the app runs (`polygloss_platform::launch::ensure_app`, which
//! launches it through LaunchServices, or `$POLYGLOSS_APP_BIN` under
//! `POLYGLOSS_TEST=1`), then send the socket op (`open` with `activate: true`).
//! Writes that do not show anything send a best-effort `store_changed` nudge to
//! an app that already runs, and never launch it.

use std::time::Duration;

use polygloss_core::ipc::{IpcClient, IpcError, Op, codes};
use polygloss_core::paths::DataPaths;
use polygloss_platform::launch::{LaunchOutcome, Launcher, SystemLauncher, ensure_app};

use crate::output::CliError;

/// How long the app may take to answer `open` (it may be resolving a review).
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a `store_changed` nudge may take (as the MCP server's).
pub const NUDGE_TIMEOUT: Duration = polygloss_mcp::context::NUDGE_TIMEOUT;

/// What happened with the app (the `app` field of the JSON output; the same
/// words as MCP `open_diff`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppShown {
    /// `--no-open`, or a command that does not show anything.
    Skipped,
    /// The running app opened (or focused) the tab.
    Opened,
    /// The app was launched and opened the tab.
    Launched,
}

impl AppShown {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppShown::Skipped => "skipped",
            AppShown::Opened => "opened",
            AppShown::Launched => "launched",
        }
    }
}

/// Makes sure the app runs, launching it in the foreground if needed, and sends
/// it `op`. An app that cannot be started or does not answer is
/// `app_unavailable`; an error the app returns keeps its code.
pub fn show(paths: &DataPaths, op: Op) -> Result<AppShown, CliError> {
    show_with(paths, op, &SystemLauncher::from_env())
}

/// [`show`] with an explicit launcher.
pub fn show_with(paths: &DataPaths, op: Op, launcher: &dyn Launcher) -> Result<AppShown, CliError> {
    let shown = match ensure_app(paths, None, true, launcher) {
        LaunchOutcome::AlreadyRunning => AppShown::Opened,
        LaunchOutcome::Launched => AppShown::Launched,
        LaunchOutcome::Unavailable(message) => {
            return Err(CliError::new("app_unavailable", message));
        }
    };
    let mut client = IpcClient::connect(paths)
        .map_err(unavailable)?
        .ok_or_else(|| CliError::new("app_unavailable", "Polygloss stopped listening"))?;
    client.call(op, OPEN_TIMEOUT).map_err(from_app)?;
    Ok(shown)
}

/// Tells a running app that the store changed (up to the latest event), so it
/// shows the change now rather than at its next poll. Best effort: no app, a
/// stale socket or a slow app are all ignored.
pub fn nudge(paths: &DataPaths, seq: i64) {
    if let Ok(Some(mut client)) = IpcClient::connect(paths) {
        let _ = client.call(Op::StoreChanged { seq }, NUDGE_TIMEOUT);
    }
}

/// Connection-level failures mean the app is not usable right now.
fn unavailable(e: IpcError) -> CliError {
    CliError::new(
        "app_unavailable",
        format!("cannot reach Polygloss: {}", e.message),
    )
}

/// An answer from the app: transport failures are `app_unavailable`, the
/// app's own errors keep their code.
fn from_app(e: IpcError) -> CliError {
    let transport = [
        codes::UNAVAILABLE,
        codes::TIMEOUT,
        codes::DISCONNECTED,
        codes::IO,
    ];
    if transport.contains(&e.code.as_str()) {
        unavailable(e)
    } else {
        CliError::new(&e.code, e.message)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use polygloss_core::ipc::{ServerConfig, serve_at};
    use serde_json::json;

    use super::*;

    /// Records launches and never starts anything.
    struct NoLaunch(AtomicUsize);

    impl Launcher for NoLaunch {
        fn launch(&self, _url: Option<&str>, _activate: bool) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::other("no app in this test"))
        }
    }

    fn paths(root: &std::path::Path) -> DataPaths {
        let data = root.join("d");
        std::fs::create_dir_all(&data).expect("mkdir");
        DataPaths::resolve_with(|k| match k {
            "HOME" => Some(root.as_os_str().to_owned()),
            "POLYGLOSS_DATA_DIR" => Some(data.as_os_str().to_owned()),
            _ => None,
        })
        .expect("paths")
    }

    /// A short temp dir (the socket path must fit in `sun_path`).
    fn short_tmp() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("pgc-")
            .tempdir_in("/tmp")
            .expect("temp dir")
    }

    #[test]
    fn a_running_app_gets_the_op_and_nothing_is_launched() {
        let root = short_tmp();
        let paths = paths(root.path());
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let _server = serve_at(
            &polygloss_core::ipc::socket_path(&paths),
            ServerConfig::from_lookup(|_| None),
            move |op| {
                log.lock().expect("lock").push(op);
                Ok(json!({ "status": "opened" }))
            },
        )
        .expect("serve");
        let launcher = NoLaunch(AtomicUsize::new(0));
        let op = Op::Open {
            review_id: Some("r".into()),
            diff_id: None,
            activate: true,
        };
        assert_eq!(
            show_with(&paths, op.clone(), &launcher).expect("show"),
            AppShown::Opened
        );
        assert_eq!(launcher.0.load(Ordering::SeqCst), 0);
        assert_eq!(*seen.lock().expect("lock"), vec![op]);
    }

    #[test]
    fn no_app_and_a_failed_launch_is_app_unavailable() {
        let root = short_tmp();
        let paths = paths(root.path());
        let launcher = NoLaunch(AtomicUsize::new(0));
        let err = show_with(
            &paths,
            Op::Open {
                review_id: None,
                diff_id: None,
                activate: true,
            },
            &launcher,
        )
        .expect_err("unavailable");
        assert_eq!(err.code, "app_unavailable");
        assert!(err.message.contains("no app in this test"), "{err:?}");
        assert_eq!(launcher.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn app_errors_keep_their_code() {
        assert_eq!(
            from_app(IpcError::new(codes::NOT_FOUND, "no such review")).code,
            "not_found"
        );
        for code in [
            codes::TIMEOUT,
            codes::DISCONNECTED,
            codes::IO,
            codes::UNAVAILABLE,
        ] {
            assert_eq!(from_app(IpcError::new(code, "x")).code, "app_unavailable");
        }
    }
}
