//! Auth contracts — register / login / me / forgot-password / reset-password.
//!
//! Migrated from docbit/src/contracts/auth.rs with UserView changes:
//! - `id` uses String (UUID)
//! - `role: String` removed
//! - `roles: Vec<String>` added (multi-role RBAC)
//! - `created_at` changed from String to i64

use rust_webx::*;
use serde::{Deserialize, Serialize};

// ── Register ──

#[derive(Default, Deserialize)]
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
    pub roles: Vec<String>,
    pub created_at: i64,
}

#[post("/api/auth/register")]
impl IRequest<AuthResponse> for RegisterRequest {}

// ── Login ──

#[derive(Default, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}

// ── Auth Me (get current user from token) ──

#[claims]
#[derive(Default, Deserialize)]
pub struct AuthMeRequest;

/// Returns the current user's info based on the JWT token claims.
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}

// ── Forgot / reset password ──

#[derive(Default, Deserialize)]
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

#[derive(Default, Deserialize)]
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
