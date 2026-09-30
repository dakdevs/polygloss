//! `reply` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ApiError;

/// `reply` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplyRequest {
    pub thread_id: String,
    /// Markdown.
    pub body_md: String,
    /// Also resolve the thread (default false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve: Option<bool>,
}

/// Publishes a reply at once.
pub fn reply(_ctx: &ApiContext, _req: ReplyRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("reply"))
}
