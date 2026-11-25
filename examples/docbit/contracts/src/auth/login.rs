//! `POST /api/auth/login` — exchange credentials for a JWT.

use serde::Deserialize;
use webx::*;

use super::models::AuthResponse;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}
