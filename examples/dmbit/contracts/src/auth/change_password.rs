//! `POST /api/auth/change-password` — rotate the current principal's password.

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ChangePasswordRequest {
    /// Trusted identity from the token; the body can never name another user.
    #[fromclaim]
    pub user: IdentityUser,
    pub old_password: String,
    #[validate(custom = "validate_new_password")]
    pub new_password: String,
}

/// Field-level rule (declarative, so the failure is a `400 problem+json` field
/// error before the handler runs, with no second hand-written check inside it).
fn validate_new_password(value: &str) -> std::result::Result<(), String> {
    if value.chars().count() < 6 {
        return Err("新密码至少 6 位".to_string());
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ChangePasswordResponse {
    pub message: String,
}

#[post("/api/auth/change-password")]
#[authorize]
impl IRequest<ChangePasswordResponse> for ChangePasswordRequest {}
