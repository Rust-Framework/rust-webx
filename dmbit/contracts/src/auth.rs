//! Auth contracts — login / me / change-password.

use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserView,
}

#[derive(Serialize)]
pub struct UserView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub roles: Vec<String>,
    pub created_at: i64,
}

#[derive(Default, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct AuthMeRequest;

#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}

#[claims]
#[derive(Default, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Serialize)]
pub struct ChangePasswordResponse {
    pub message: String,
}

#[post("/api/auth/change-password")]
#[authorize]
impl IRequest<ChangePasswordResponse> for ChangePasswordRequest {}
