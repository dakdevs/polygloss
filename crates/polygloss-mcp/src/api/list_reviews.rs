//! `list_reviews` (design §15.2). Implemented by T4.5.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::shapes::AssignedFilter;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `list_reviews` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListReviewsRequest {
    /// Only reviews of the repository containing this path (omit for all repos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Only reviews with this status: `open`, `changes_requested`, `commented`,
    /// `approved` or `rereview_requested`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `any` (default) or `me`: only reviews assigned to this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned: Option<AssignedFilter>,
    /// `next_cursor` from the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Lists reviews, most recently active first.
pub fn list_reviews(_ctx: &ApiContext, _req: ListReviewsRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("list_reviews"))
}
