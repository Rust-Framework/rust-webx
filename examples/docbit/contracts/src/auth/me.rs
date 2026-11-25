//! `GET /api/auth/me` — the current principal's profile.

use serde::Deserialize;
use webx::*;

use super::models::UserModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct AuthMeRequest;

/// Returns the current user's info based on the JWT token claims.
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserModel> for AuthMeRequest {}
