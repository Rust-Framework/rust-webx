//! Blog entity — DB-backed blog post with FKs to Category and User.

use rust_ef::prelude::*;

use super::category::Category;
use super::comment::Comment;
use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("blogs")]
pub struct Blog {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
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
    pub tags: String, // JSON 数组字符串，如 ["rust","web"]
    #[required]
    #[foreign_key(Category)]
    #[index]
    pub category_id: i32,
    #[required]
    #[foreign_key(User)]
    #[index]
    pub author_id: i32,
    #[required]
    pub published_at: i64,
    #[required]
    pub created_at: i64,
    #[required]
    pub updated_at: i64,
    #[navigation]
    pub category: BelongsTo<Category>,
    #[navigation]
    pub author: BelongsTo<User>,
    #[navigation]
    pub comments: HasMany<Comment>,
}
