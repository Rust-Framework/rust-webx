//! `GET /api/auth/me` — the current principal.

use serde::Deserialize;
use webx::*;

use super::models::UserModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct AuthMeRequest {
    /// Trusted identity from the token; the body/query can never supply it.
    #[fromclaim]
    pub user: IdentityUser,
}

#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserModel> for AuthMeRequest {}
