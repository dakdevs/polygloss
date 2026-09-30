//! `list_threads` (design §15.2). Implemented by T4.5.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::shapes::{AuthorFilter, ThreadKindParam, ThreadStatusFilter};
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `list_threads` params. Pass `review_id` or `diff_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListThreadsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    /// A diff id or an unambiguous prefix of at least 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_id: Option<String>,
    /// `open` (default), `resolved` or `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ThreadStatusFilter>,
    /// Who started the thread: `human`, `agent` or `any` (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<AuthorFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ThreadKindParam>,
    /// Only threads on this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Only threads with activity after this event seq (`latest_seq` / `next_since`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// `next_cursor` from the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Lists the threads of a review or diff that agents may see.
pub fn list_threads(_ctx: &ApiContext, _req: ListThreadsRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("list_threads"))
}
