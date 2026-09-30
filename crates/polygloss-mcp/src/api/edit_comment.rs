//! `edit_comment` (design §15.2, §8.2): the caller's own comments only, i.e.
//! agent comments with the caller's `author_name` (OQ-30; any session of the
//! same client counts); anyone else's is `forbidden`. A published comment gets
//! `edited_at` and `comment.edited`.

use polygloss_core::review::Viewer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::shapes::timestamp;
use crate::app_link;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// `edit_comment` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditCommentRequest {
    pub comment_id: String,
    /// The new markdown body.
    pub body_md: String,
}

/// `edit_comment` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditCommentResult {
    pub comment_id: String,
    pub edited_at: String,
}

/// Edits one of the caller's own comments.
pub fn edit_comment(
    ctx: &ApiContext,
    req: EditCommentRequest,
) -> Result<EditCommentResult, ApiError> {
    ctx.core
        .edit_comment(&req.comment_id, &req.body_md, &ctx.author())?;
    app_link::after_write(ctx);
    let comment = ctx.core.comment(&req.comment_id, Viewer::Agent)?;
    let edited_at = comment
        .edited_at
        .ok_or_else(|| ApiError::internal("the edit recorded no edited_at"))?;
    Ok(EditCommentResult {
        comment_id: req.comment_id,
        edited_at: timestamp(edited_at),
    })
}
