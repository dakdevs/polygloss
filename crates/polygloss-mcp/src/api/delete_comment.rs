//! `delete_comment` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ApiError;

/// `delete_comment` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeleteCommentRequest {
    pub comment_id: String,
}

/// Deletes one of the caller's own comments.
pub fn delete_comment(_ctx: &ApiContext, _req: DeleteCommentRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("delete_comment"))
}
