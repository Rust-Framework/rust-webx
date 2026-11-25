//! `POST /api/session` — exchange a bearer token for browser session cookies.
//!
//! The endpoint authenticates the caller through the normal bearer pipeline and
//! answers with a self-controlled `Response`: one `Set-Cookie` for the
//! `HttpOnly` session cookie and one for the readable CSRF cookie. From then on
//! a browser upgrades `/ws/im` with those cookies (it cannot set
//! `Authorization` on a WebSocket handshake), and the CSRF cookie is mirrored in
//! `X-CSRF-Token` on unsafe HTTP methods.
//!
//! A normal DTO response cannot add response headers, which is why the route is
//! registered as `IRequest<Response>` (ASP.NET Core's `IActionResult` return
//! type).

use serde::{Deserialize, Serialize};
use webx::*;

/// Bearer-authenticated caller asking for a browser session.
#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateSessionRequest {
    /// Bearer identity; the framework answers `401` when the request is
    /// anonymous (the endpoint never reads an identity out of the body).
    #[fromclaim]
    pub user: IdentityUser,
}

/// Confirmation body. The cookies are the actual payload (`Set-Cookie`).
#[derive(Debug, Clone, Serialize)]
pub struct SessionResponse {
    /// Subject the session cookie now authenticates.
    pub user_id: String,
    /// Display name the session claims (`name`), falling back to the id.
    pub username: String,
}

impl SessionResponse {
    /// Render the caller's verified identity for the browser.
    pub fn from_user(user: &IdentityUser) -> Self {
        Self {
            user_id: user.id.clone(),
            username: user.display_name().to_string(),
        }
    }
}

#[post("/api/session")]
impl IRequest<Response> for CreateSessionRequest {}
