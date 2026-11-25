//! `POST /api/auth/register` — self-service sign-up.

use serde::Deserialize;
use webx::*;

use super::models::AuthResponse;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct RegisterRequest {
    pub name: String,
    pub email: String,
    pub password: String,
}

#[post("/api/auth/register")]
impl IRequest<AuthResponse> for RegisterRequest {}
