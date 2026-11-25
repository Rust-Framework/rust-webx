//! `POST /api/auth/forgot-password` — start a password reset.

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct ForgotPasswordResponse {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_token: Option<String>,
}

#[post("/api/auth/forgot-password")]
impl IRequest<ForgotPasswordResponse> for ForgotPasswordRequest {}
