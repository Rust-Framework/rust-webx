//! `POST /api/login` — password login through the framework security ports.
//!
//! The reference app registers an unknown username on first login (there is no
//! separate sign-up endpoint); a known username must prove its password.

use serde::{Deserialize, Serialize};
use webx::*;

/// Anonymous request: the credentials travel in the body, so the endpoint
/// declares no identity field.
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct LoginRequest {
    /// Trimmed by the handler before use.
    #[validate(length(min = 1, max = 64))]
    pub username: String,
    /// Reference minimum; the framework hasher decides the real strength.
    #[validate(length(min = 4))]
    pub password: String,
}

/// The bearer token the client presents on HTTP and on the WebSocket upgrade.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user_id: String,
    pub username: String,
}

#[post("/api/login")]
impl IRequest<LoginResponse> for LoginRequest {}
