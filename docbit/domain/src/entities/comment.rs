//! Comment entity — dual self-referencing FKs for reply and quote.

use rust_ef::prelude::*;

use super::blog::Blog;
use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("comments")]
pub struct Comment {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(Blog)]
    #[index]
    #[max_length(36)]
    pub blog_id: String,
    #[required]
    #[foreign_key(User)]
    #[index]
    #[max_length(36)]
    pub user_id: String,
    #[required]
    #[max_length(100)]
    pub user_name: String,
    #[required]
    pub content: String,
    #[foreign_key(Comment)]
    #[max_length(36)]
    pub parent_id: Option<String>,
    #[foreign_key]
    #[max_length(36)]
    pub quoted_id: Option<String>,
    #[required]
    pub created_at: i64,
    #[index]
    #[max_length(36)]
    pub updated_id: Option<String>,
    #[required]
    pub updated_at: i64,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub blog: BelongsTo<Blog>,
    #[navigation]
    pub user: BelongsTo<User>,
    #[navigation]
    pub parent: BelongsTo<Comment>,
    #[navigation]
    pub quoted: BelongsTo<Comment>,
}
