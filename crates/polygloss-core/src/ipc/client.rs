//! Blocking socket client used by the CLI and MCP server (T4.1).
//!
//! T4.4 placeholder with T4.1's interface (`connect`, `call`): JSON Lines over
//! `paths.socket`. T4.1's module replaces this file on merge.

use std::io::{BufRead as _, BufReader, ErrorKind, Write as _};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::{Value, json};

use super::protocol::{IpcError, Op};
use crate::paths::DataPaths;

/// A connection to the app's socket.
#[derive(Debug)]
pub struct IpcClient {
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl IpcClient {
    /// `Ok(None)`: no app is running.
    pub fn connect(paths: &DataPaths) -> Result<Option<IpcClient>, IpcError> {
        match UnixStream::connect(&paths.socket) {
            Ok(stream) => Ok(Some(IpcClient {
                reader: BufReader::new(stream),
                next_id: 1,
            })),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                Ok(None)
            }
            Err(e) => Err(IpcError::new("io", format!("connecting: {e}"))),
        }
    }

    /// Sends `op` and waits up to `timeout` for its response.
    pub fn call(&mut self, op: Op, timeout: Duration) -> Result<Value, IpcError> {
        let io = |e: std::io::Error| IpcError::new("io", e.to_string());
        let id = self.next_id;
        self.next_id += 1;
        let mut req = serde_json::to_value(&op).map_err(|e| IpcError::new("io", e.to_string()))?;
        req["v"] = json!(1);
        req["id"] = json!(id);
        let stream = self.reader.get_mut();
        stream.set_read_timeout(Some(timeout)).map_err(io)?;
        stream.set_write_timeout(Some(timeout)).map_err(io)?;
        let mut line = req.to_string();
        line.push('\n');
        stream.write_all(line.as_bytes()).map_err(io)?;
        let mut resp = String::new();
        self.reader.read_line(&mut resp).map_err(io)?;
        let v: Value =
            serde_json::from_str(&resp).map_err(|e| IpcError::new("bad_response", e.to_string()))?;
        if v["ok"] == json!(true) {
            Ok(v["result"].clone())
        } else {
            Err(serde_json::from_value(v["error"].clone())
                .unwrap_or_else(|_| IpcError::new("bad_response", resp.trim())))
        }
    }
}
