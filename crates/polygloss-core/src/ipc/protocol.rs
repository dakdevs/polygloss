//! Socket protocol request and response types (T4.1, design §13.3).
//!
//! T4.4 placeholder: only what `polygloss-mcp` needs (`Op`, `IpcError`), with
//! T4.1's names and wire shapes. T4.1's module replaces this file on merge.

use polygloss_diff::Side;
use serde::{Deserialize, Serialize};

/// One operation (design §13.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Hello {
        client: String,
    },
    Open {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        review_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_id: Option<String>,
        #[serde(default)]
        activate: bool,
    },
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
    StoreChanged {
        seq: i64,
    },
    DebugState,
}

/// A protocol error: on the wire `{"code","message"}`.
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
}
