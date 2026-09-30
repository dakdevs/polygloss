//! `get_thread` (design §15.2). Implemented by T4.5.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ApiError;

/// `get_thread` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetThreadRequest {
    pub thread_id: String,
}

/// One thread with its anchor, snippets, `diff_hunk` and comments.
pub fn get_thread(_ctx: &ApiContext, _req: GetThreadRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("get_thread"))
}
