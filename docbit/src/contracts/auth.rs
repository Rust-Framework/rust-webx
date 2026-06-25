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
