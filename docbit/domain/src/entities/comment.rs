//! Comment entity — dual self-referencing FKs for reply (`parent_id`) and quote (`quoted_id`).
//!
//! 双自外键规避：`parent_id` 保留命名 `#[foreign_key(Comment)]` 生成 `FK_Comment` 常量；
//! `quoted_id` 用裸 `#[foreign_key]`（无参数），跳过 `FK_` 常量生成，避免 E0592 重复定义。
//! 运行时勿在 `linq!` 中 `include b.parent`/`b.quoted`（宏多 FK 导航绑定缺陷会按错误列关联），
//! 改为查询后按 `parent_id`/`quoted_id` 二次查询手动装配。

use rust_ef::prelude::*;

use super::blog::Blog;
use super::user::User;

#[derive(Debug, Clone, EntityType)]
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
    pub parent_id: Option<i32>, // 回复目标评论 FK（直接回复），命名 FK 常量
    #[foreign_key]
    pub quoted_id: Option<i32>, // 引用评论 FK（块引用），裸形式跳过 FK_ 常量生成
    #[required]
    pub created_at: i64,
    #[index]
    pub updated_id: Option<i32>, // 无 FK
    #[required]
    pub updated_at: i64,
    #[required]
    #[index]
    pub is_deleted: bool, // 审核隐藏
    #[navigation]
    pub blog: BelongsTo<Blog>,
    #[navigation]
    pub user: BelongsTo<User>,
    #[navigation]
    pub parent: BelongsTo<Comment>, // 运行时勿 include，手动二次查询
    #[navigation]
    pub quoted: BelongsTo<Comment>, // 运行时勿 include，手动二次查询
}
