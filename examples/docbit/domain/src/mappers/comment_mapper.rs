//! Comment mappers — `Comment` ↔ `CommentModel`.

use docbit_contracts::comment::{CommentModel, CreateCommentRequest};
use rust_ef::prelude::*;
use webx_core::Error;

use crate::audit::operator_id;
use crate::entities::Comment;
use crate::ids::new_id;
use crate::mappers::now_secs;

impl TryFrom<CreateCommentRequest> for Comment {
    type Error = Error;

    fn try_from(req: CreateCommentRequest) -> Result<Self, Self::Error> {
        let content = req.content.trim().to_string();
        if content.is_empty() {
            return Err(Error::validation("Comment cannot be empty"));
        }
        if content.len() > 4000 {
            return Err(Error::validation("Comment too long"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(Comment {
            id: new_id(),
            blog_id: req.blog_id,
            user_id: op.clone().unwrap_or_default(),
            user_name: String::new(),
            content,
            parent_id: req.parent_id,
            quoted_id: req.quoted_id,
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            blog: BelongsTo::new(),
            user: BelongsTo::new(),
            parent: BelongsTo::new(),
            quoted: BelongsTo::new(),
        })
    }
}

impl From<&Comment> for CommentModel {
    fn from(e: &Comment) -> Self {
        Self {
            id: e.id.clone(),
            blog_id: e.blog_id.clone(),
            user_id: e.user_id.clone(),
            user_name: e.user_name.clone(),
            content: e.content.clone(),
            parent_id: e.parent_id.clone(),
            quoted_id: e.quoted_id.clone(),
            created_at: e.created_at,
        }
    }
}
