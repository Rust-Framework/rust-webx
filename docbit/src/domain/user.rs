use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

/// User database entity.
#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("users")]
pub struct UserEntity {
    #[primary_key]
    pub id: String,
    #[max_length(200)]
    pub name: String,
    #[max_length(200)]
    pub email: String,
    /// Bcrypt-hashed password.
    pub password_hash: String,
    pub role: String,
    pub created_at: String,
}

/// DTO returned to API clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub password_hash: String,
    pub role: String,
    pub created_at: String,
}

impl From<UserEntity> for UserModel {
    fn from(e: UserEntity) -> Self {
        Self {
            id: e.id,
            name: e.name,
            email: e.email,
            password_hash: e.password_hash,
            role: e.role,
            created_at: e.created_at,
        }
    }
}
