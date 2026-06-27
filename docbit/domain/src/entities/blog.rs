//! Blog entity — DB-backed blog post with FKs to Category and User.
//!
//! `author_id` 是博客作者；`created_id`/`updated_id` 是运维审计操作人，语义分离。

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
    #[index]
    pub created_id: Option<i32>, // 运维审计创建人（无 FK）
    #[index]
    pub updated_id: Option<i32>, // 运维审计更新人（无 FK）
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
