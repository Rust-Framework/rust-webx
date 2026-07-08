//! Blog entity — DB-backed blog post with FKs to Category and User.

use rust_ef::prelude::*;

use super::category::Category;
use super::comment::Comment;
use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("blogs")]
pub struct Blog {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(200)]
    #[unique]
    pub slug: String,
    #[required]
    #[max_length(200)]
    pub title: String,
    #[max_length(500)]
    pub summary: String,
    #[required]
    pub content: String,
    #[required]
    pub tags: String,
    #[required]
    #[foreign_key(Category)]
    #[index]
    #[max_length(36)]
    pub category_id: String,
    #[required]
    #[foreign_key(User)]
    #[index]
    #[max_length(36)]
    pub author_id: String,
    #[required]
    pub published_at: i64,
    #[required]
    pub created_at: i64,
    #[required]
    pub updated_at: i64,
    #[index]
    #[max_length(36)]
    pub created_id: Option<String>,
    #[index]
    #[max_length(36)]
    pub updated_id: Option<String>,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub category: BelongsTo<Category>,
    #[navigation]
    pub author: BelongsTo<User>,
    #[navigation]
    pub comments: HasMany<Comment>,
}
