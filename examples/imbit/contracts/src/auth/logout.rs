//! `POST /api/logout` — revoke the caller's own token.
//!
//! Revocation is effectful only when the host registers a session store (the
//! host calls `use_session_store`); the token id is bound from the verified
//! JWT, never from the body.

use serde::Deserialize;
use webx::*;

/// Identity plus the `jti` of the caller's own bearer token.
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct LogoutRequest {
    /// Authenticated caller (the framework answers `401` when anonymous).
    #[fromclaim]
    pub user: IdentityUser,
    /// `jti` claim of the presented token — the revocation key.
    #[fromclaim("jti")]
    pub jti: String,
}

#[post("/api/logout")]
impl IRequest<OperationResult> for LogoutRequest {}
