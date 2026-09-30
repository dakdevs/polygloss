//! `request_rereview` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ApiError;

/// `request_rereview` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestRereviewRequest {
    pub review_id: String,
    /// What changed since the last submission (markdown).
    pub summary_md: String,
}

/// Pins the live state as a new iteration and asks the human to review again.
pub fn request_rereview(
    _ctx: &ApiContext,
    _req: RequestRereviewRequest,
) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("request_rereview"))
}
