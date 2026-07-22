//! Role + RoleUser entities.

use rust_ef::prelude::*;

use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("roles")]
pub struct Role {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(50)]
    #[unique]
    pub name: String,
    #[max_length(200)]
    pub description: String,
    #[index]
    #[max_length(36)]
    pub created_id: Option<String>,
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
    pub users: HasMany<User, RoleUser>,
}

#[derive(Debug, Clone, EntityType)]
#[table("role_users")]
pub struct RoleUser {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[foreign_key(User)]
    #[index]
    #[max_length(36)]
    pub user_id: String,
    #[required]
    #[foreign_key(Role)]
    #[index]
    #[max_length(36)]
    pub role_id: String,
    #[required]
    pub created_at: i64,
}
