use rust_webapp::*;
use serde::{Deserialize, Serialize};

// â”€â”€ Register â”€â”€

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    pub email: String,
    pub password: String,
}

/// Registration response includes a JWT token and user info.
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
    pub role: String,
    pub created_at: String,
}

#[post("/api/auth/register")]
impl IRequest<AuthResponse> for RegisterRequest {}

// â”€â”€ Login â”€â”€

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}

// â”€â”€ Auth Me (get current user from token) â”€â”€

pub struct AuthMeRequest;

/// Returns the current user's info based on the JWT token claims.
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}

// ── Forgot / reset password ──

#[derive(Deserialize)]
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

#[derive(Deserialize)]
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
