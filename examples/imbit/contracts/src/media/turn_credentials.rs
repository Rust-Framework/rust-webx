//! `GET /api/turn` — ephemeral TURN/STUN credentials for the caller.
//!
//! Credentials are derived from the framework `Turn` configuration section
//! (coturn REST API); the client uses them for the media plane that the
//! `/ws/im` signaling channel negotiates. The response type is the framework's
//! `TurnCredentials`, so no DTO is duplicated here.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct TurnCredentialsRequest {
    #[fromclaim]
    pub user: IdentityUser,
    /// Requested lifetime; the handler clamps it to the framework bounds.
    #[fromquery]
    pub ttl_seconds: Option<u64>,
}

#[get("/api/turn")]
impl IRequest<TurnCredentials> for TurnCredentialsRequest {}
