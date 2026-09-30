//! Blocking socket client used by the CLI and MCP server (T4.1).
//!
//! [`IpcClient::connect`] returns `None` when no app listens (no socket
//! file, or a stale one nobody accepts on), so callers can launch the app or
//! report it unavailable. [`IpcClient::call`] sends one request and waits for
//! its response. A call that times out or fails midway leaves the connection
//! unusable; later calls return `disconnected`.

use std::io::{BufRead as _, BufReader, ErrorKind, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, Stream, ToFsName as _};
use serde_json::Value;

use super::protocol::{IpcError, Op, PROTOCOL_VERSION, Request, Response, codes};
use crate::paths::DataPaths;

/// A connection to the app's socket.
pub struct IpcClient {
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
    /// there (no file, or connection refused).
    pub fn connect_at(path: &Path) -> Result<Option<IpcClient>, IpcError> {
        let name = path
            .to_fs_name::<GenericFilePath>()
            .map_err(|e| IpcError::io("naming the socket", &e))?;
        match Stream::connect(name) {
            Ok(stream) => Ok(Some(IpcClient {
                reader: BufReader::new(stream),
                next_id: 1,
                broken: false,
            })),
            Err(e) if no_listener(&e) => Ok(None),
            Err(e) => Err(IpcError::io(
                &format!("connecting to {}", path.display()),
                &e,
            )),
        }
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
        let result = self.exchange(op, timeout);
        if result.as_ref().is_err_and(|e| {
            [codes::TIMEOUT, codes::IO, codes::DISCONNECTED].contains(&e.code.as_str())
        }) {
            self.broken = true;
        }
        result
    }

    fn exchange(&mut self, op: Op, timeout: Duration) -> Result<Value, IpcError> {
        let deadline = Instant::now() + timeout;
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
            .map_err(|e| IpcError::io("setting the send timeout", &e))?;
        let mut writer: &Stream = stream;
        writer
            .write_all(line.as_bytes())
            .map_err(|e| IpcError::io("sending the request", &e))?;
        loop {
            let response = self.read_response(deadline)?;
            // `id` 0: the server rejected the connection before reading.
            if response.id == id || (response.id == 0 && !response.ok) {
                return response.into_result();
            }
        }
    }

    fn read_response(&mut self, deadline: Instant) -> Result<Response, IpcError> {
        self.reader
            .get_ref()
            .set_recv_timeout(Some(remaining(deadline)?))
            .map_err(|e| IpcError::io("setting the receive timeout", &e))?;
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| IpcError::io("reading the response", &e))?;
        if n == 0 {
            return Err(IpcError::new(
                codes::DISCONNECTED,
                "the app closed the connection",
            ));
        }
        serde_json::from_str(&line)
            .map_err(|e| IpcError::new(codes::IO, format!("unreadable response: {e}")))
    }
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
