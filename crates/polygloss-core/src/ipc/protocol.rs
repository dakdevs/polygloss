//! Socket protocol request and response types (T4.1, design §13.3).
//!
//! JSON Lines: one request object per line, one response line per request.
//!
//! ```text
//! → {"v":1,"id":7,"op":"focus","review_id":"…","path":"src/a.rs","side":"new","line":12}
//! ← {"id":7,"ok":true,"result":{"status":"focused"}}
//! ← {"id":7,"ok":false,"error":{"code":"not_found","message":"…"}}
//! ```
//!
//! Lines are 1-based (the store's and agents' convention). A response whose
//! request could not be read at all (bad JSON, a rejected peer) has `id` 0.

use polygloss_diff::Side;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version every request carries in `v`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Environment variable that enables the test-only surface (plan OQ-P4):
/// the `debug_state` op answers only when it is `1` in the app's environment.
pub const TEST_ENV: &str = "POLYGLOSS_TEST";

/// The longest request line the server reads (1 MiB). Longer lines are
/// answered with `bad_request` and the connection is closed.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// One operation (design §13.3). Serialized with the op name in `op`
/// (`"hello"`, `"open"`, `"focus"`, `"store_changed"`, `"debug_state"`) and
/// its parameters next to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Answered by the server itself: `{app, version, pid, protocol}`.
    Hello { client: String },
    /// Open or focus the tab of a review (`review_id`), or of the most recent
    /// review of a diff (`diff_id`); with neither, only bring the app forward
    /// (when `activate`). `activate` brings the app to the front.
    Open {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        review_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_id: Option<String>,
        #[serde(default)]
        activate: bool,
    },
    /// Scroll to a location (`path`, `side` default new, 1-based `line`) or a
    /// thread, opening the tab if needed. Never opens an external editor.
    Focus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        review_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        side: Option<Side>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
    },
    /// Nudge: the store changed (up to event `seq`); read events now.
    StoreChanged { seq: i64 },
    /// Test-only (`POLYGLOSS_TEST=1`): the app's state as
    /// `{tabs: [{review_id, diff_id, anchor, cursor}], focused_tab, banners, badge, events_seen}`.
    DebugState,
}

/// Every op name, in declaration order.
pub const OP_NAMES: [&str; 5] = ["hello", "open", "focus", "store_changed", "debug_state"];

impl Op {
    /// The op's wire name (`"store_changed"`).
    pub fn name(&self) -> &'static str {
        match self {
            Op::Hello { .. } => "hello",
            Op::Open { .. } => "open",
            Op::Focus { .. } => "focus",
            Op::StoreChanged { .. } => "store_changed",
            Op::DebugState => "debug_state",
        }
    }
}

/// A request line: `{"v":1,"id":7,"op":"…",…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Always [`PROTOCOL_VERSION`].
    pub v: u32,
    /// Chosen by the client; echoed in the response.
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// A response line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcError>,
}

impl Response {
    /// `{"id":…,"ok":true,"result":…}`.
    pub fn success(id: u64, result: Value) -> Response {
        Response {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// `{"id":…,"ok":false,"error":{…}}`.
    pub fn failure(id: u64, error: IpcError) -> Response {
        Response {
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }

    /// The result (`null` when absent), or the error.
    pub fn into_result(self) -> Result<Value, IpcError> {
        if self.ok {
            Ok(self.result.unwrap_or(Value::Null))
        } else {
            Err(self.error.unwrap_or_else(|| {
                IpcError::new(
                    codes::INTERNAL,
                    "the app answered ok=false without an error",
                )
            }))
        }
    }
}

/// A protocol error: on the wire `{"code","message"}`; also the error type of
/// the client and server functions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> IpcError {
        IpcError {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    /// An I/O failure while `what` (code [`codes::IO`], or [`codes::TIMEOUT`]
    /// for a timed-out read or write).
    pub fn io(what: &str, err: &std::io::Error) -> IpcError {
        let code = match err.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => codes::TIMEOUT,
            _ => codes::IO,
        };
        IpcError::new(code, format!("{what}: {err}"))
    }
}

/// The error codes of the socket protocol.
pub mod codes {
    /// The `op` is not one this app knows (or a test-only op outside tests).
    pub const UNKNOWN_OP: &str = "unknown_op";
    /// The line is not a valid request (bad JSON, missing or mistyped fields).
    pub const BAD_REQUEST: &str = "bad_request";
    /// `v` is not [`super::PROTOCOL_VERSION`].
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    /// The peer runs as another user.
    pub const FORBIDDEN: &str = "forbidden";
    /// No such review, diff or thread.
    pub const NOT_FOUND: &str = "not_found";
    /// No answer in time.
    pub const TIMEOUT: &str = "timeout";
    /// A socket or filesystem error.
    pub const IO: &str = "io";
    /// The other side closed the connection.
    pub const DISCONNECTED: &str = "disconnected";
    /// Another app already serves this socket.
    pub const ALREADY_RUNNING: &str = "already_running";
    /// The app is quitting or could not carry the request out.
    pub const UNAVAILABLE: &str = "unavailable";
    /// Anything else.
    pub const INTERNAL: &str = "internal";
}

/// Parses one request line. On failure returns the id to answer with (the
/// request's when it could be read, else 0) and the error: `bad_request`,
/// `unsupported_version` or `unknown_op`.
pub fn parse_request(line: &str) -> Result<Request, (u64, IpcError)> {
    let value: Value = serde_json::from_str(line).map_err(|e| {
        (
            0,
            IpcError::new(codes::BAD_REQUEST, format!("not JSON: {e}")),
        )
    })?;
    let Some(obj) = value.as_object() else {
        return Err((
            0,
            IpcError::new(codes::BAD_REQUEST, "a request must be a JSON object"),
        ));
    };
    let id = obj
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| (0, IpcError::new(codes::BAD_REQUEST, "`id` must be a u64")))?;
    match obj.get("v").and_then(Value::as_u64) {
        Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
        Some(v) => {
            return Err((
                id,
                IpcError::new(
                    codes::UNSUPPORTED_VERSION,
                    format!(
                        "protocol version {v} is not supported (this app speaks {PROTOCOL_VERSION})"
                    ),
                ),
            ));
        }
        None => {
            return Err((
                id,
                IpcError::new(codes::BAD_REQUEST, "`v` must be a number"),
            ));
        }
    }
    let op = match obj.get("op") {
        Some(Value::String(op)) => op.clone(),
        _ => {
            return Err((
                id,
                IpcError::new(codes::BAD_REQUEST, "`op` must be a string"),
            ));
        }
    };
    if !OP_NAMES.contains(&op.as_str()) {
        return Err((
            id,
            IpcError::new(codes::UNKNOWN_OP, format!("unknown op {op:?}")),
        ));
    }
    serde_json::from_value::<Request>(value).map_err(|e| {
        (
            id,
            IpcError::new(codes::BAD_REQUEST, format!("bad `{op}` request: {e}")),
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn request_wire_format_matches_design() {
        let req = Request {
            v: PROTOCOL_VERSION,
            id: 7,
            op: Op::Focus {
                review_id: Some("r1".into()),
                diff_id: None,
                path: Some("src/a.rs".into()),
                side: Some(Side::New),
                line: Some(12),
                thread_id: None,
            },
        };
        let wire = serde_json::to_value(&req).unwrap();
        assert_eq!(
            wire,
            json!({"v":1,"id":7,"op":"focus","review_id":"r1","path":"src/a.rs","side":"new","line":12})
        );
        assert_eq!(parse_request(&wire.to_string()).unwrap(), req);
        let unit = json!({"v":1,"id":2,"op":"debug_state"}).to_string();
        assert_eq!(parse_request(&unit).unwrap().op, Op::DebugState);
        let open = json!({"v":1,"id":3,"op":"open","review_id":"r"}).to_string();
        assert_eq!(
            parse_request(&open).unwrap().op,
            Op::Open {
                review_id: Some("r".into()),
                diff_id: None,
                activate: false
            }
        );
    }

    #[test]
    fn response_wire_format_matches_design() {
        let ok = serde_json::to_value(Response::success(7, json!({"a":1}))).unwrap();
        assert_eq!(ok, json!({"id":7,"ok":true,"result":{"a":1}}));
        let err = Response::failure(7, IpcError::new(codes::NOT_FOUND, "gone"));
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"id":7,"ok":false,"error":{"code":"not_found","message":"gone"}})
        );
        assert_eq!(
            err.into_result().unwrap_err().code,
            codes::NOT_FOUND.to_owned()
        );
    }

    #[test]
    fn parse_request_classifies_errors() {
        let code = |line: &str| parse_request(line).unwrap_err();
        assert_eq!(code("{").1.code, codes::BAD_REQUEST);
        assert_eq!(code("{").0, 0);
        assert_eq!(code("[1]").1.code, codes::BAD_REQUEST);
        assert_eq!(code(r#"{"v":1,"op":"hello"}"#).1.code, codes::BAD_REQUEST);
        let (id, e) = code(r#"{"v":2,"id":5,"op":"hello","client":"x"}"#);
        assert_eq!((id, e.code.as_str()), (5, codes::UNSUPPORTED_VERSION));
        let (id, e) = code(r#"{"v":1,"id":6,"op":"frobnicate"}"#);
        assert_eq!((id, e.code.as_str()), (6, codes::UNKNOWN_OP));
        let (id, e) = code(r#"{"v":1,"id":8,"op":"store_changed","seq":"x"}"#);
        assert_eq!((id, e.code.as_str()), (8, codes::BAD_REQUEST));
    }

    #[test]
    fn op_names_cover_every_op() {
        let ops = [
            Op::Hello { client: "c".into() },
            Op::Open {
                review_id: None,
                diff_id: None,
                activate: true,
            },
            Op::Focus {
                review_id: None,
                diff_id: None,
                path: None,
                side: None,
                line: None,
                thread_id: None,
            },
            Op::StoreChanged { seq: 1 },
            Op::DebugState,
        ];
        let names: Vec<&str> = ops.iter().map(Op::name).collect();
        assert_eq!(names, OP_NAMES);
        for op in ops {
            let wire = serde_json::to_value(&op).unwrap();
            assert_eq!(wire["op"], op.name());
        }
    }
}
