use rust_ef::prelude::*;
use serde::{Deserialize, Serialize};

use crate::contracts::user::UserModel;

#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("users")]
pub struct UserEntity {
    #[primary_key]
    pub id: String,
    #[max_length(200)]
    pub name: String,
    #[max_length(200)]
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub created_at: String,
}

#[derive(EntityType, Clone, Serialize, Deserialize, Debug)]
#[table("password_reset_tokens")]
pub struct PasswordResetTokenEntity {
    #[primary_key]
    pub token: String,
    pub user_id: String,
    pub expires_at: String,
    pub used: i64,
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
