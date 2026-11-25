//! PasswordResetToken entity — supports forgot/reset password flow.

use rust_ef::prelude::*;

use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("password_reset_tokens")]
pub struct PasswordResetToken {
    #[primary_key]
    #[max_length(36)]
    pub id: String,
    #[required]
    #[max_length(200)]
    #[unique]
    pub token: String,
    #[required]
    #[foreign_key(User)]
    #[max_length(36)]
    pub user_id: String,
    #[required]
    pub expires_at: i64,
    #[required]
    pub used: i32,
}
