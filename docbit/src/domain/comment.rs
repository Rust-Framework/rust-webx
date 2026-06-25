use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

use crate::contracts::blog::BlogCommentModel;

#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("blog_comments")]
pub struct BlogCommentEntity {
    #[primary_key]
    pub id: String,
    #[max_length(120)]
    pub post_slug: String,
    pub user_id: String,
    #[max_length(120)]
    pub user_name: String,
    pub content: String,
    pub created_at: String,
}

impl From<BlogCommentEntity> for BlogCommentModel {
    fn from(e: BlogCommentEntity) -> Self {
        Self {
            id: e.id,
            post_slug: e.post_slug,
            user_id: e.user_id,
            user_name: e.user_name,
            content: e.content,
            created_at: e.created_at,
        }
    }
}
