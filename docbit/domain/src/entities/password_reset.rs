//! PasswordResetToken entity — supports forgot/reset password flow.

use rust_ef::prelude::*;

use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("password_reset_tokens")]
pub struct PasswordResetToken {
    #[primary_key]
    #[auto_increment]
    pub id: i32,
    #[required]
    #[max_length(200)]
    #[unique]
    pub token: String,
    #[required]
    #[foreign_key(User)]
    pub user_id: i32,
    #[required]
    pub expires_at: i64,
    #[required]
    pub used: i32, // 0 = 未使用, 1 = 已使用
}
