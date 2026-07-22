//! User entity.

use rust_ef::prelude::*;

use super::role::{Role, RoleUser};

#[derive(Debug, Clone, EntityType)]
#[table("users")]
pub struct User {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
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
    pub roles: HasMany<Role, RoleUser>,
}
