//! `POST /api/auth/reset-password` — redeem a reset token.

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct ResetPasswordResponse {
    pub message: String,
}

#[post("/api/auth/reset-password")]
impl IRequest<ResetPasswordResponse> for ResetPasswordRequest {}
