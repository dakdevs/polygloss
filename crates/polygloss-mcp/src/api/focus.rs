//! `focus` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::shapes::SideParam;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `focus` params. Pass `diff_id` or `review_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FocusRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<SideParam>,
    /// 1-based line on `side`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Scrolls the Polygloss app to a location, launching it if needed.
pub fn focus(_ctx: &ApiContext, _req: FocusRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("focus"))
}
