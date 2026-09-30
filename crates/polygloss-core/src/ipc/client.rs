//! Blocking socket client used by the CLI and MCP server (T4.1).
//!
//! [`IpcClient::connect`] returns `None` when no app listens (no socket
//! file, or a stale one nobody accepts on), so callers can launch the app or
//! report it unavailable. Before connecting it checks that the socket and its
//! directory belong to us ([`check_socket_owner`]; the `$TMPDIR`/`/tmp`
//! fallback of design §13.2 lives in a shared directory). [`IpcClient::call`]
//! sends one request and waits for its response.
//!
//! - The server closes a connection that stays idle for its idle timeout
//!   (60 s). A call that finds the connection closed by the app before any
//!   answer (end of stream, broken pipe, reset) reconnects once and sends the
//!   request again, so long-lived clients survive idle periods.
//! - A call that times out or fails otherwise leaves the connection unusable;
//!   later calls return `disconnected`.

use std::io::{BufRead as _, BufReader, ErrorKind, Write as _};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, Stream, ToFsName as _};
use serde_json::Value;

use super::protocol::{IpcError, Op, PROTOCOL_VERSION, Request, Response, codes};
use crate::paths::DataPaths;

/// A connection to the app's socket.
pub struct IpcClient {
    path: PathBuf,
    reader: BufReader<Stream>,
    next_id: u64,
    broken: bool,
}

impl std::fmt::Debug for IpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcClient")
            .field("next_id", &self.next_id)
            .field("broken", &self.broken)
            .finish()
    }
}

impl IpcClient {
    /// Connects to the app of `paths` ([`super::socket_path`]). `Ok(None)`:
    /// no app is running.
    pub fn connect(paths: &DataPaths) -> Result<Option<IpcClient>, IpcError> {
        IpcClient::connect_at(&super::socket_path(paths))
    }

    /// Connects to the socket at `path`. `Ok(None)` when nothing listens
    /// there (no file, or connection refused); `forbidden` when the socket or
    /// its directory is not ours ([`check_socket_owner`]).
    pub fn connect_at(path: &Path) -> Result<Option<IpcClient>, IpcError> {
        Ok(open_stream(path)?.map(|stream| IpcClient {
            path: path.to_path_buf(),
            reader: BufReader::new(stream),
            next_id: 1,
            broken: false,
        }))
    }

    /// Sends `op` and waits up to `timeout` for its response: the result, or
    /// the app's error (`code`, `message`), or `timeout`, `disconnected`,
    /// `io`.
    pub fn call(&mut self, op: Op, timeout: Duration) -> Result<Value, IpcError> {
        if self.broken {
            return Err(IpcError::new(
                codes::DISCONNECTED,
                "the connection failed earlier",
            ));
        }
        let deadline = Instant::now() + timeout;
        let mut result = self.exchange(op.clone(), deadline);
        if let Err(Exchange::PeerClosed(_)) = &result {
            // Most likely the app closed an idle connection: reconnect once.
            result = match self.reconnect() {
                Ok(()) => self.exchange(op, deadline),
                Err(e) => Err(Exchange::Failed(e)),
            };
        }
        let result = result.map_err(Exchange::into_error);
        if result.as_ref().is_err_and(|e| {
            [codes::TIMEOUT, codes::IO, codes::DISCONNECTED].contains(&e.code.as_str())
        }) {
            self.broken = true;
        }
        result
    }

    /// A fresh connection to the same socket.
    fn reconnect(&mut self) -> Result<(), IpcError> {
        let stream = open_stream(&self.path)?
            .ok_or_else(|| IpcError::new(codes::DISCONNECTED, "the app stopped listening"))?;
        self.reader = BufReader::new(stream);
        Ok(())
    }

    fn exchange(&mut self, op: Op, deadline: Instant) -> Result<Value, Exchange> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_string(&Request {
            v: PROTOCOL_VERSION,
            id,
            op,
        })
        .map_err(|e| IpcError::new(codes::INTERNAL, format!("encoding the request: {e}")))?;
        line.push('\n');
        let stream = self.reader.get_ref();
        stream
            .set_send_timeout(Some(remaining(deadline)?))
            .map_err(|e| Exchange::dead_socket("setting the send timeout", &e))?;
        let mut writer: &Stream = stream;
        writer
            .write_all(line.as_bytes())
            .map_err(|e| Exchange::io("sending the request", &e))?;
        loop {
            let response = self.read_response(deadline)?;
            // `id` 0: the server rejected the connection before reading.
            if response.id == id || (response.id == 0 && !response.ok) {
                return response.into_result().map_err(Exchange::Failed);
            }
        }
    }

    fn read_response(&mut self, deadline: Instant) -> Result<Response, Exchange> {
        self.reader
            .get_ref()
            .set_recv_timeout(Some(remaining(deadline)?))
            .map_err(|e| Exchange::dead_socket("setting the receive timeout", &e))?;
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| Exchange::io("reading the response", &e))?;
        if n == 0 {
            return Err(Exchange::PeerClosed(IpcError::new(
                codes::DISCONNECTED,
                "the app closed the connection",
            )));
        }
        serde_json::from_str(&line)
            .map_err(|e| IpcError::new(codes::IO, format!("unreadable response: {e}")).into())
    }
}

/// How one request/response exchange failed.
enum Exchange {
    /// The app closed the connection before answering (idle close): the
    /// request may be sent again on a new connection.
    PeerClosed(IpcError),
    Failed(IpcError),
}

impl Exchange {
    /// An I/O error; a closed or reset connection is [`Exchange::PeerClosed`].
    fn io(what: &str, err: &std::io::Error) -> Exchange {
        let closed = matches!(
            err.kind(),
            ErrorKind::BrokenPipe
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::NotConnected
                | ErrorKind::UnexpectedEof
        );
        let e = IpcError::io(what, err);
        if closed {
            Exchange::PeerClosed(IpcError::new(codes::DISCONNECTED, e.message))
        } else {
            Exchange::Failed(e)
        }
    }

    /// A failed `setsockopt`: macOS answers `EINVAL` once the peer closed the
    /// connection, so the request can go out again on a new one.
    fn dead_socket(what: &str, err: &std::io::Error) -> Exchange {
        Exchange::PeerClosed(IpcError::new(codes::DISCONNECTED, format!("{what}: {err}")))
    }

    fn into_error(self) -> IpcError {
        match self {
            Exchange::PeerClosed(e) | Exchange::Failed(e) => e,
        }
    }
}

impl From<IpcError> for Exchange {
    fn from(e: IpcError) -> Exchange {
        Exchange::Failed(e)
    }
}

/// Checks the socket's ownership, then connects. `Ok(None)`: nothing listens.
fn open_stream(path: &Path) -> Result<Option<Stream>, IpcError> {
    match check_socket_owner(path, super::current_uid()) {
        Ok(()) => {}
        Err(e) if e.code == codes::NOT_FOUND => return Ok(None),
        Err(e) => return Err(e),
    }
    let name = path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| IpcError::io("naming the socket", &e))?;
    match Stream::connect(name) {
        Ok(stream) => Ok(Some(stream)),
        Err(e) if no_listener(&e) => Ok(None),
        Err(e) => Err(IpcError::io(
            &format!("connecting to {}", path.display()),
            &e,
        )),
    }
}

/// Before anything is sent: `path` must be a socket (not a symlink) owned by
/// `uid`, in a directory owned by `uid` that no one else may write to.
/// `forbidden` otherwise; `not_found` when there is no socket file.
pub fn check_socket_owner(path: &Path, uid: u32) -> Result<(), IpcError> {
    let forbidden = |what: String| {
        IpcError::new(
            codes::FORBIDDEN,
            format!("refusing {}: {what}", path.display()),
        )
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(IpcError::new(codes::NOT_FOUND, "no socket file"));
        }
        Err(e) => return Err(IpcError::io(&format!("reading {}", path.display()), &e)),
    };
    if !meta.file_type().is_socket() {
        return Err(forbidden("it is not a socket".into()));
    }
    if meta.uid() != uid {
        return Err(forbidden(format!(
            "it belongs to uid {}, not {uid}",
            meta.uid()
        )));
    }
    let Some(dir) = path.parent() else {
        return Err(forbidden("it has no directory".into()));
    };
    let dir_meta = std::fs::metadata(dir)
        .map_err(|e| IpcError::io(&format!("reading {}", dir.display()), &e))?;
    if dir_meta.uid() != uid {
        return Err(forbidden(format!(
            "its directory belongs to uid {}, not {uid}",
            dir_meta.uid()
        )));
    }
    if dir_meta.permissions().mode() & 0o022 != 0 {
        return Err(forbidden("others may write to its directory".into()));
    }
    Ok(())
}

/// The time left before `deadline` (never zero: a zero socket timeout means
/// "block forever"), or `timeout` once it passed.
fn remaining(deadline: Instant) -> Result<Duration, IpcError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(IpcError::new(
            codes::TIMEOUT,
            "no answer from the app in time",
        ));
    }
    Ok(left.max(Duration::from_millis(1)))
}

/// Connecting failed because nothing listens: no socket file, a stale one,
/// or something that is not a socket.
fn no_listener(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::NotFound | ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset
    )
}
