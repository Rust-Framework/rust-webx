//! `GET /api/turn` — ephemeral TURN/STUN credentials for the caller.
//!
//! The `Turn` configuration section is the single source of truth
//! (`Turn.Secret`, `Turn.Urls`, `Turn.TtlSeconds`, `APP__Turn__*`); the host
//! validates it at startup and registers `TurnRestConfig`, so the handler only
//! clamps the requested lifetime and derives the coturn REST credentials.

use std::sync::Arc;

use webx::*;

use imbit_contracts::media::TurnCredentialsRequest;

#[derive(Inject)]
pub struct TurnCredentialsHandler {
    #[inject]
    config: Arc<TurnRestConfig>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<TurnCredentialsRequest, TurnCredentials> for TurnCredentialsHandler {
    async fn handle(&mut self, request: TurnCredentialsRequest) -> ActionResult<TurnCredentials> {
        let mut config = (*self.config).clone();
        if let Some(ttl) = request.ttl_seconds {
            config.ttl_seconds = ttl.clamp(1, MAX_TURN_TTL_SECONDS);
        }

        Ok(config.credentials_for(&request.user.id)?)
    }
}
