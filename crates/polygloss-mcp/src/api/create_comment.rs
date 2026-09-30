//! `create_comment` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::shapes::{AgentThreadKind, AnchorParam};
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `create_comment` params. Pass `review_id` or `diff_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateCommentRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_id: Option<String>,
    /// `note` explains a change; `question` asks the human for a decision.
    pub kind: AgentThreadKind,
    /// Markdown.
    pub body_md: String,
    /// Omit for a review-level comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorParam>,
}

/// Creates a published note or question thread.
pub fn create_comment(_ctx: &ApiContext, _req: CreateCommentRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("create_comment"))
}
