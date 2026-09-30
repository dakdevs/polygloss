//! Agent-facing errors (design §15.1 "Results"). Every `api::*` function returns
//! [`ApiError`]; the MCP server turns it into an `isError: true` tool result and the
//! JSON CLI into `{ "error": { "code", "message" } }`.

use std::fmt;

use polygloss_core::review::CoreError;
use serde::{Deserialize, Serialize};

/// The error codes of design §15.1, serialized in snake case (`not_found`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    /// No such review, diff, thread or comment; also a malformed or ambiguous diff
    /// id prefix (the message lists the matches).
    NotFound,
    RepoNotFound,
    ObjectsMissing,
    InvalidAnchor,
    CapExceeded,
    /// Editing or deleting a comment that is not the caller's (OQ-30).
    Forbidden,
    AppUnavailable,
    /// The stored state or the rules reject the request as given (e.g. an empty
    /// body, a bad cursor): change the request rather than retry.
    Conflict,
    /// Anything else: store, filesystem or git failures.
    Internal,
}

impl ApiErrorCode {
    /// Every code, in declaration order.
    pub const ALL: [ApiErrorCode; 9] = [
        ApiErrorCode::NotFound,
        ApiErrorCode::RepoNotFound,
        ApiErrorCode::ObjectsMissing,
        ApiErrorCode::InvalidAnchor,
        ApiErrorCode::CapExceeded,
        ApiErrorCode::Forbidden,
        ApiErrorCode::AppUnavailable,
        ApiErrorCode::Conflict,
        ApiErrorCode::Internal,
    ];

    /// The wire form (`not_found`, …).
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiErrorCode::NotFound => "not_found",
            ApiErrorCode::RepoNotFound => "repo_not_found",
            ApiErrorCode::ObjectsMissing => "objects_missing",
            ApiErrorCode::InvalidAnchor => "invalid_anchor",
            ApiErrorCode::CapExceeded => "cap_exceeded",
            ApiErrorCode::Forbidden => "forbidden",
            ApiErrorCode::AppUnavailable => "app_unavailable",
            ApiErrorCode::Conflict => "conflict",
            ApiErrorCode::Internal => "internal",
        }
    }

    /// Inverse of [`ApiErrorCode::as_str`].
    pub fn parse(s: &str) -> Option<ApiErrorCode> {
        ApiErrorCode::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

impl fmt::Display for ApiErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An agent-facing error: `{code, message}` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> ApiError {
        ApiError {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(ApiErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> ApiError {
        ApiError::new(ApiErrorCode::Conflict, message)
    }

    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(ApiErrorCode::Internal, message)
    }

    /// The tool exists in the skeleton (T4.4) but its task has not landed yet.
    pub fn not_implemented(tool: &str) -> ApiError {
        ApiError::internal(format!("{tool} is not implemented in this build"))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<CoreError> for ApiError {
    /// Maps through [`CoreError::code`], so core and the agent surface agree.
    fn from(e: CoreError) -> ApiError {
        let code = ApiErrorCode::parse(e.code()).unwrap_or(ApiErrorCode::Internal);
        ApiError::new(code, e.to_string())
    }
}
