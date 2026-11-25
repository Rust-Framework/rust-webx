//! `POST /api/login` — verify (or bootstrap) the password, then mint a token.
//!
//! Crypto goes through the framework ports: `IPasswordHasher` verifies/hashes,
//! `ITokenIssuer` signs the JWT the client presents on HTTP and on the
//! WebSocket upgrade.

use std::sync::Arc;

use webx::*;

use imbit_contracts::auth::{LoginRequest, LoginResponse};
use imbit_domain::ImStore;

#[derive(Inject)]
pub struct LoginHandler {
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, LoginResponse> for LoginHandler {
    async fn handle(&mut self, request: LoginRequest) -> ActionResult<LoginResponse> {
        let username = request.username.trim();
        if username.is_empty() {
            return Err(Error::validation("username is required"));
        }

        // Login doubles as sign-up for an unknown username (reference app only).
        let user_id = match self.store.credentials(username) {
            Some((user_id, hash)) => {
                if !self.hasher.verify(&request.password, &hash).await? {
                    return Err(Error::unauthorized("invalid credentials"));
                }
                user_id
            }
            None => {
                let hash = self.hasher.hash(&request.password).await?;
                self.store.register_user(username, hash)
            }
        };

        let issued = self
            .tokens
            .issue(TokenClaims::new(&user_id).with_claim("name", username.to_string()))
            .await?;

        Ok(LoginResponse {
            token: issued.token,
            user_id,
            username: username.to_string(),
        })
    }
}
