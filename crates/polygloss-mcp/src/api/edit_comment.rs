//! `edit_comment` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ApiError;

/// `edit_comment` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditCommentRequest {
    pub comment_id: String,
    /// The new markdown body.
    pub body_md: String,
}

/// Edits one of the caller's own comments.
pub fn edit_comment(_ctx: &ApiContext, _req: EditCommentRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("edit_comment"))
}
