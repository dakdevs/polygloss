//! One app per data dir for unbundled builds (design §13.4; a bundled app
//! also has LaunchServices).
//!
//! [`claim`] takes an exclusive `flock` on `<data_dir>/app.lock` for the
//! life of the process. When another process holds it, or when an app
//! already answers on the socket (one that predates the lock, or runs from
//! another build), this process is [`Claim::Secondary`]: [`forward`] opens
//! the review its argv names (`Core::open`, like the CLI) and asks the
//! running app to show it (`open` with `activate: true`; plain `Polygloss`
//! only brings the app forward), and [`forward_urls`] hands over its
//! `polygloss://` arguments as the socket ops they map to ([`url_op`]);
//! then the process exits 0. A running app that is still starting gets
//! [`FORWARD_WAIT`] to bind its socket.

use std::fs::{DirBuilder, File, OpenOptions, TryLockError};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::time::{Duration, Instant};

use polygloss_core::ipc::{IpcClient, Op};
use polygloss_core::paths::DataPaths;
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::urls::{PolyglossUrl, UrlError, parse_url};
use serde_json::Value;

/// How long a second instance waits for the running app's socket.
pub const FORWARD_WAIT: Duration = Duration::from_secs(10);

/// How long a second instance waits for the running app to show the review.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(30);

/// Held by the one running app; dropping it (or exiting) releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Whether this process is the app or should hand over to it.
#[derive(Debug)]
pub enum Claim {
    Primary(InstanceLock),
    Secondary,
}

/// Claims the data dir of `paths` for this process (module docs).
pub fn claim(paths: &DataPaths) -> std::io::Result<Claim> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&paths.data_dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&paths.app_lock)?;
    match file.try_lock() {
        Ok(()) if app_answers(paths) => Ok(Claim::Secondary),
        Ok(()) => Ok(Claim::Primary(InstanceLock { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(Claim::Secondary),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// An app answers `hello` on the socket of `paths`.
fn app_answers(paths: &DataPaths) -> bool {
    let Ok(Some(mut client)) = IpcClient::connect(paths) else {
        return false;
    };
    client
        .call(
            Op::Hello {
                client: "Polygloss".into(),
            },
            Duration::from_secs(2),
        )
        .is_ok()
}

/// Hands `open` (the review named on the command line, if any) to the
/// running app of `paths`, waiting up to `wait` for its socket. Returns the
/// app's answer.
pub fn forward(
    open: Option<&OpenRequest>,
    paths: &DataPaths,
    wait: Duration,
) -> Result<Value, String> {
    let review_id = match open {
        Some(req) => {
            let core = Core::with_paths(paths.clone()).map_err(|e| e.to_string())?;
            Some(core.open(req).map_err(|e| e.to_string())?.review_id)
        }
        None => None,
    };
    let op = Op::Open {
        review_id,
        diff_id: None,
        activate: true,
    };
    send(op, paths, wait)
}

/// The socket op that shows `url` in the running app: `open` for a review
/// or a diff alone, `focus` for a diff position or a thread. Like a URL the
/// app gets from LaunchServices, it does not activate the app (a clicked
/// link comes forward on its own).
pub fn url_op(url: &str) -> Result<Op, UrlError> {
    Ok(match parse_url(url)? {
        PolyglossUrl::Review(review_id) => Op::Open {
            review_id: Some(review_id),
            diff_id: None,
            activate: false,
        },
        PolyglossUrl::Diff {
            diff_id,
            path: None,
            ..
        } => Op::Open {
            review_id: None,
            diff_id: Some(diff_id),
            activate: false,
        },
        PolyglossUrl::Diff {
            diff_id,
            path,
            side,
            line,
        } => Op::Focus {
            review_id: None,
            diff_id: Some(diff_id),
            path,
            side,
            line,
            thread_id: None,
        },
        PolyglossUrl::Thread(thread_id) => Op::Focus {
            review_id: None,
            diff_id: None,
            path: None,
            side: None,
            line: None,
            thread_id: Some(thread_id),
        },
    })
}

/// Hands `urls` to the running app of `paths`, in order ([`url_op`]),
/// waiting up to `wait` for its socket. Stops at the first URL that does
/// not parse or that the app rejects.
pub fn forward_urls(urls: &[String], paths: &DataPaths, wait: Duration) -> Result<(), String> {
    for url in urls {
        let op = url_op(url).map_err(|e| e.to_string())?;
        send(op, paths, wait).map_err(|e| format!("{url}: {e}"))?;
    }
    Ok(())
}

/// Sends `op` to the running app of `paths`, waiting up to `wait` for its
/// socket.
fn send(op: Op, paths: &DataPaths, wait: Duration) -> Result<Value, String> {
    let deadline = Instant::now() + wait;
    loop {
        match IpcClient::connect(paths) {
            Ok(Some(mut client)) => {
                return client
                    .call(op, FORWARD_TIMEOUT)
                    .map_err(|e| format!("the running app answered: {e}"));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                return Err(format!(
                    "another Polygloss holds {} but does not answer on its socket",
                    paths.app_lock.display()
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}
