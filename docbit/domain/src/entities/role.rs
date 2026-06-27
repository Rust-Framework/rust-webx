//! Role + RoleUser entities — many-to-many between User and Role.

use rust_ef::prelude::*;

use super::resource::{Authorize, Resource};
use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("roles")]
pub struct Role {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(50)]
    #[unique]
    pub name: String,
    #[max_length(200)]
    pub description: String,
    #[index]
    pub created_id: Option<i32>, // 无 FK
    #[required]
    pub created_at: i64,
    #[index]
    pub updated_id: Option<i32>, // 无 FK
    #[required]
    pub updated_at: i64,
    #[required]
    #[index]
    pub is_deleted: bool,
    #[navigation]
    pub users: HasMany<User, RoleUser>,
    #[navigation]
    pub resources: HasMany<Resource, Authorize>,
}

#[derive(Debug, Clone, EntityType)]
#[table("role_users")]
pub struct RoleUser {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[foreign_key(User)]
    #[index]
    pub user_id: i32,
    #[required]
    #[foreign_key(Role)]
    #[index]
    pub role_id: i32,
    #[required]
    pub created_at: i64, // 联结表仅留 created_at
}
