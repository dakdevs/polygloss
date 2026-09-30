//! `resolve` and `unresolve` (design §15.2). Implemented by T4.6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ApiError;

/// `resolve` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolveRequest {
    pub thread_id: String,
    /// Optional closing reply (markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_md: Option<String>,
}

/// `unresolve` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnresolveRequest {
    pub thread_id: String,
}

/// Resolves a thread, optionally with a closing reply.
pub fn resolve(_ctx: &ApiContext, _req: ResolveRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("resolve"))
}

/// Reopens a resolved thread.
pub fn unresolve(_ctx: &ApiContext, _req: UnresolveRequest) -> Result<Value, ApiError> {
    Err(ApiError::not_implemented("unresolve"))
}
