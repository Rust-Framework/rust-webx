//! User entity — i32 auto-increment PK, multi-role via RoleUser join.

use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

use super::role::{Role, RoleUser};

#[derive(Debug, Clone, EntityType, Serialize, Deserialize)]
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
    #[required]
    pub created_at: i64,
    #[navigation]
    pub roles: HasMany<Role, RoleUser>,
}
