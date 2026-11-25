//! `POST /api/auth/login` — exchange credentials for a token.

use serde::{Deserialize, Serialize};
use webx::*;

use super::models::UserModel;

/// The issued token plus the principal it belongs to.
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserModel,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}
