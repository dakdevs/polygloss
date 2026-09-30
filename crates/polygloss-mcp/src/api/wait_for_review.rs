//! `wait_for_review` (design §15.2). Implemented by T4.7.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::WaitControl;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `wait_for_review` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WaitForReviewRequest {
    pub review_id: String,
    /// Event seq to wait after (`next_since` of the previous call). Default: now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Seconds to wait (default and max 1500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u32>,
}

/// Blocks until the human submits or archives the review, or the timeout.
pub fn wait_for_review(
    _ctx: &ApiContext,
    _req: WaitForReviewRequest,
    _ctl: &WaitControl,
) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("wait_for_review"))
}
