//! `delete_comment` (design §15.2, §8.2): the caller's own comments only
//! (`forbidden` otherwise, OQ-30). A root with replies leaves a "comment
//! deleted" placeholder; a thread left without comments is deleted.
//! `comment.deleted` either way.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::app_link;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `delete_comment` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeleteCommentRequest {
    pub comment_id: String,
}

/// `delete_comment` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeleteCommentResult {
    pub comment_id: String,
    /// Always true.
    pub deleted: bool,
    /// The thread shows a "comment deleted" placeholder (a deleted root that
    /// still has replies).
    pub placeholder: bool,
}

/// Deletes one of the caller's own comments.
pub fn delete_comment(
    ctx: &ApiContext,
    req: DeleteCommentRequest,
) -> Result<DeleteCommentResult, ApiError> {
    let deleted = ctx.core.delete_comment(&req.comment_id, &ctx.author())?;
    app_link::after_write(ctx);
    Ok(DeleteCommentResult {
        comment_id: req.comment_id,
        deleted: true,
        placeholder: deleted.placeholder,
    })
}
