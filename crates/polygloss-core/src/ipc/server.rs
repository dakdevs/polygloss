//! Socket server loop run by the app (T4.1, design §13.3).
//!
//! [`serve`] binds the socket and runs the accept loop on a `std::thread`;
//! every connection gets its own thread that reads request lines and writes
//! one response line each. Before reading anything it checks the peer's uid
//! with `getpeereid` (a different user gets one `forbidden` response with id
//! 0 and the connection is closed).
//!
//! - `hello` is answered by the server itself (`{app, version, pid,
//!   protocol}`), so it works while the app's main thread is busy.
//! - `debug_state` reaches the handler only when [`ServerConfig::allow_debug`]
//!   (`POLYGLOSS_TEST=1`); otherwise it is an `unknown_op`.
//! - Every other op goes to the handler. Handler calls run one at a time
//!   (the handler is `Send`, not `Sync`), in arrival order across
//!   connections.
//!
//! Binding: the socket's directory is created with mode `0700` (a `$TMPDIR`
//! fallback directory must also be owned by us), and the socket file is set
//! to `0600`. An existing socket file is probed with `connect` first: a live
//! server means `already_running`; a stale one (connection refused, left by
//! a crash) is replaced (`try_overwrite`). Anything at the path that is not a
//! socket is left alone and reported. Dropping the [`ServerHandle`] stops the
//! accept loop and removes the socket file if it is still the one we bound.

use std::io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Stream, ToFsName as _};
use serde_json::{Value, json};

use super::protocol::{
    IpcError, MAX_LINE_BYTES, Op, PROTOCOL_VERSION, Response, TEST_ENV, codes, parse_request,
};
use crate::paths::DataPaths;

/// How long a connection may stay idle between requests before the server
/// closes it.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How the server treats its peers and test-only ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerConfig {
    /// Pass `debug_state` to the handler (`POLYGLOSS_TEST=1`, plan OQ-P4).
    pub allow_debug: bool,
    /// The only peer uid accepted (ours; tests inject another).
    pub expected_uid: u32,
}

impl ServerConfig {
    /// From the process environment, accepting only our own uid.
    pub fn from_env() -> ServerConfig {
        ServerConfig::from_lookup(|k| std::env::var_os(k))
    }

    /// From an environment lookup (tests), accepting only our own uid.
    pub fn from_lookup(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> ServerConfig {
        ServerConfig {
            allow_debug: env(TEST_ENV).is_some_and(|v| v == "1"),
            expected_uid: super::current_uid(),
        }
    }
}

/// Accepts a connection only from `expected` (the peer's uid is `peer`).
pub fn check_peer(peer: u32, expected: u32) -> Result<(), IpcError> {
    if peer == expected {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::FORBIDDEN,
            format!("peer uid {peer} is not the app's user ({expected})"),
        ))
    }
}

/// The running server. Dropping it stops the accept loop (connections in
/// flight finish) and removes its socket file.
pub struct ServerHandle {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    /// `(dev, ino)` of the socket file we bound.
    bound: Option<(u64, u64)>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerHandle")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ServerHandle {
    /// The socket's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stops the server and waits for its accept loop to end.
    pub fn shutdown(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let ours = self.bound.is_some() && file_id(&self.path) == self.bound;
        if ours {
            // Wakes the blocking `accept`, which then sees `stop`.
            if let Ok(name) = self.path.as_path().to_fs_name::<GenericFilePath>() {
                drop(Stream::connect(name));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            let _ = std::fs::remove_file(&self.path);
        }
        // Else another server replaced our socket file: the accept thread
        // stays blocked (it holds no resources others need) until exit.
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if !self.stop.load(Ordering::SeqCst) {
            self.stop_now();
        }
    }
}

/// Serves `paths`'s socket ([`super::socket_path`]) with
/// [`ServerConfig::from_env`].
pub fn serve(
    paths: &DataPaths,
    handler: impl Fn(Op) -> Result<Value, IpcError> + Send + 'static,
) -> Result<ServerHandle, IpcError> {
    serve_with(paths, ServerConfig::from_env(), handler)
}

/// [`serve`] with an explicit configuration.
pub fn serve_with(
    paths: &DataPaths,
    config: ServerConfig,
    handler: impl Fn(Op) -> Result<Value, IpcError> + Send + 'static,
) -> Result<ServerHandle, IpcError> {
    let path = super::socket_path(paths);
    let in_data_dir = path.parent() == Some(paths.data_dir.as_path());
    prepare_dir(&path, !in_data_dir)?;
    bind_and_run(path, config, Box::new(handler))
}

/// Serves the socket at `path` (its directory must exist; it is not
/// created or checked). For test doubles of the app (fake servers bound at
/// a sandbox's socket path).
pub fn serve_at(
    path: &Path,
    config: ServerConfig,
    handler: impl Fn(Op) -> Result<Value, IpcError> + Send + 'static,
) -> Result<ServerHandle, IpcError> {
    bind_and_run(path.to_path_buf(), config, Box::new(handler))
}

type Handler = Box<dyn Fn(Op) -> Result<Value, IpcError> + Send>;

/// Creates the socket's directory `0700`. A fallback directory (not the data
/// dir) must be a real directory owned by us.
fn prepare_dir(path: &Path, fallback: bool) -> Result<(), IpcError> {
    let Some(dir) = path.parent() else {
        return Err(IpcError::new(codes::IO, "the socket path has no directory"));
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| IpcError::io(&format!("creating {}", dir.display()), &e))?;
    let meta = std::fs::symlink_metadata(dir)
        .map_err(|e| IpcError::io(&format!("reading {}", dir.display()), &e))?;
    if fallback && (!meta.is_dir() || meta.uid() != super::current_uid()) {
        return Err(IpcError::new(
            codes::FORBIDDEN,
            format!(
                "{} must be a directory owned by uid {}",
                dir.display(),
                super::current_uid()
            ),
        ));
    }
    if meta.is_dir() && meta.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| IpcError::io(&format!("chmod 0700 {}", dir.display()), &e))?;
    }
    Ok(())
}

fn bind_and_run(
    path: PathBuf,
    config: ServerConfig,
    handler: Handler,
) -> Result<ServerHandle, IpcError> {
    let stale = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_socket() => {
            if probe(&path) {
                return Err(IpcError::new(
                    codes::ALREADY_RUNNING,
                    format!("another app already serves {}", path.display()),
                ));
            }
            true
        }
        Ok(_) => {
            return Err(IpcError::new(
                codes::IO,
                format!("{} exists and is not a socket", path.display()),
            ));
        }
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => return Err(IpcError::io(&format!("reading {}", path.display()), &e)),
    };
    let name = path
        .as_path()
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| IpcError::io("naming the socket", &e))?;
    let listener = ListenerOptions::new()
        .name(name)
        .try_overwrite(stale)
        .max_spin_time(Duration::from_secs(1))
        // We remove the file ourselves, and only while it is still ours.
        .reclaim_name(false)
        .create_sync()
        .map_err(|e| match e.kind() {
            ErrorKind::AddrInUse => IpcError::new(
                codes::ALREADY_RUNNING,
                format!("another app already serves {}", path.display()),
            ),
            _ => IpcError::io(&format!("binding {}", path.display()), &e),
        })?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| IpcError::io(&format!("chmod 0600 {}", path.display()), &e))?;
    let bound = file_id(&path);
    let stop = Arc::new(AtomicBool::new(false));
    let handler = Arc::new(Mutex::new(handler));
    let thread = {
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("polygloss-ipc-accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match conn {
                        Ok(stream) => spawn_connection(stream, config, handler.clone()),
                        Err(e) => {
                            tracing::warn!("accepting a socket connection: {e}");
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            })
            .map_err(|e| IpcError::io("starting the accept thread", &e))?
    };
    Ok(ServerHandle {
        path,
        stop,
        bound,
        thread: Some(thread),
    })
}

/// A socket answers at `path` (another app is running).
fn probe(path: &Path) -> bool {
    path.to_fs_name::<GenericFilePath>()
        .is_ok_and(|name| Stream::connect(name).is_ok())
}

fn file_id(path: &Path) -> Option<(u64, u64)> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.dev(), m.ino()))
}

fn spawn_connection(stream: Stream, config: ServerConfig, handler: Arc<Mutex<Handler>>) {
    let spawned = std::thread::Builder::new()
        .name("polygloss-ipc-conn".into())
        .spawn(move || {
            if let Err(e) = connection(stream, config, &handler) {
                tracing::debug!("socket connection ended: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("starting a socket connection thread: {e}");
    }
}

/// Serves one connection until the peer closes it, stays idle for
/// [`IDLE_TIMEOUT`], or sends an oversized line.
fn connection(
    stream: Stream,
    config: ServerConfig,
    handler: &Mutex<Handler>,
) -> std::io::Result<()> {
    let peer = peer_uid(&stream)?;
    if let Err(e) = check_peer(peer, config.expected_uid) {
        tracing::warn!("rejected a socket connection: {e}");
        return write_response(&stream, &Response::failure(0, e));
    }
    stream.set_recv_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_send_timeout(Some(IDLE_TIMEOUT))?;
    let mut reader = BufReader::new(&stream);
    loop {
        let mut buf = Vec::new();
        let n = (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if buf.len() > MAX_LINE_BYTES {
            let e = IpcError::new(
                codes::BAD_REQUEST,
                format!("request lines are limited to {MAX_LINE_BYTES} bytes"),
            );
            return write_response(&stream, &Response::failure(0, e));
        }
        let response = match std::str::from_utf8(&buf) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => respond(line, config, handler),
            Err(_) => Response::failure(0, IpcError::new(codes::BAD_REQUEST, "not UTF-8")),
        };
        write_response(&stream, &response)?;
    }
}

/// The response to one request line.
fn respond(line: &str, config: ServerConfig, handler: &Mutex<Handler>) -> Response {
    let req = match parse_request(line) {
        Ok(req) => req,
        Err((id, e)) => return Response::failure(id, e),
    };
    let result = match req.op {
        Op::Hello { .. } => Ok(hello()),
        Op::DebugState if !config.allow_debug => Err(IpcError::new(
            codes::UNKNOWN_OP,
            format!("debug_state needs {TEST_ENV}=1"),
        )),
        op => match handler.lock() {
            Ok(handler) => handler(op),
            Err(_) => Err(IpcError::new(
                codes::INTERNAL,
                "the socket handler panicked",
            )),
        },
    };
    match result {
        Ok(value) => Response::success(req.id, value),
        Err(e) => Response::failure(req.id, e),
    }
}

/// `hello`'s result: the app version, pid and protocol version.
pub fn hello() -> Value {
    json!({
        "app": "polygloss",
        "version": crate::VERSION,
        "pid": std::process::id(),
        "protocol": PROTOCOL_VERSION,
    })
}

fn write_response(mut stream: &Stream, response: &Response) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(response).map_err(std::io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)
}

/// The effective uid of the peer of `stream` (`getpeereid(2)`).
#[allow(unsafe_code)]
fn peer_uid(stream: &Stream) -> std::io::Result<u32> {
    let Stream::UdSocket(uds) = stream;
    let fd = uds.inner().as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `fd` is an open socket borrowed from `stream` for this call, and
    // both out-pointers are valid, writable locals.
    let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    if rc == 0 {
        Ok(uid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}
