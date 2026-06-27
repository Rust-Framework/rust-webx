//! Comment entity — dual self-referencing FKs for reply (`parent_id`) and quote (`quoted_id`).

use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

use super::blog::Blog;
use super::user::User;

#[derive(Debug, Clone, EntityType, Serialize, Deserialize)]
#[table("comments")]
pub struct Comment {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[foreign_key(Blog)]
    #[index]
    pub blog_id: i32,
    #[required]
    #[foreign_key(User)]
    #[index]
    pub user_id: i32,
    #[required]
    #[max_length(100)]
    pub user_name: String, // 评论者昵称冗余，避免 JOIN
    #[required]
    pub content: String,
    #[foreign_key(Comment)]
    pub parent_id: Option<i32>, // 回复目标评论 FK（直接回复）
    #[foreign_key(Comment)]
    pub quoted_id: Option<i32>, // 引用评论 FK（块引用）
    #[required]
    pub created_at: i64,
    #[navigation]
    pub blog: BelongsTo<Blog>,
    #[navigation]
    pub user: BelongsTo<User>,
    #[navigation]
    pub parent: BelongsTo<Comment>,
    #[navigation]
    pub quoted: BelongsTo<Comment>,
}
