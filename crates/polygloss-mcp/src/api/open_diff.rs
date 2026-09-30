//! `open_diff` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::shapes::SourceParam;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `open_diff` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenDiffRequest {
    /// Any path inside the repository. Default: the client's first root that is a
    /// git worktree, else the project directory, else the server's cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// What to diff. Default: `{kind: "live"}` (working tree vs merge-base).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceParam>,
    /// Display label such as `PR #123`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Open the review in the Polygloss app (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<bool>,
    /// Assign the review to this session so its submission wakes you (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign: Option<bool>,
}

/// Resolves the source, pins a live state, records the review and iteration,
/// assigns it to the caller and asks the app to show it.
pub fn open_diff(_ctx: &ApiContext, _req: OpenDiffRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("open_diff"))
}
