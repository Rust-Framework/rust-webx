//! User entity — i32 auto-increment PK, multi-role via RoleUser join.
//!
//! 审计字段（created_id/updated_id）不加 `#[foreign_key]`，避免 User 表自引用
//! 产生重复 `FK_User` 常量，且软删除用户后审计记录仍可读。

use rust_ef::prelude::*;

use super::role::{Role, RoleUser};

#[derive(Debug, Clone, EntityType)]
#[table("users")]
pub struct User {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(100)]
    pub name: String,
    #[required]
    #[max_length(200)]
    #[unique]
    pub email: String,
    #[required]
    #[max_length(200)]
    pub password_hash: String,
    #[index]
    pub created_id: Option<i32>, // 创建人（首条 admin 为 None），无 FK
    #[required]
    pub created_at: i64,
    #[index]
    pub updated_id: Option<i32>, // 更新人，无 FK
    #[required]
    pub updated_at: i64,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub roles: HasMany<Role, RoleUser>,
}
