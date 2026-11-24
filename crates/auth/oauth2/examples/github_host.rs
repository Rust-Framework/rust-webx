//! Copy-paste GitHub OAuth login mounted directly on a rust-webx `Host`.
//!
//! ```powershell
//! $env:GITHUB_CLIENT_ID = "..."
//! $env:GITHUB_CLIENT_SECRET = "..."
//! $env:GITHUB_REDIRECT_URI = "http://localhost:5000/auth/github/callback"
//! cargo run -p rust-webx-oauth2 --example github_host
//! ```
//!
//! The two endpoints are:
//! - `GET /auth/github/start`    → 302 to GitHub
//! - `GET /auth/github/callback` → code→token→user→local JWT
//!
//! `PendingStore` is in-memory for the example. Production stores
//! `state -> pkce_verifier` in a session or `OneTimeSecret` and deletes it on
//! first use.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use webx::*;
use webx_oauth2::{OAuth2Client, OAuth2Config, OAuth2Provider, OAuth2User};

/// One-shot OAuth state store. Replace with session/OneTimeSecret in production.
#[derive(Default)]
struct PendingStore(Mutex<HashMap<String, Option<String>>>);

#[derive(WebxRequestMeta)]
struct GitHubStart;

#[get("/auth/github/start")]
impl IRequest<Response> for GitHubStart {}

#[derive(Inject)]
struct GitHubStartHandler {
    #[inject]
    client: Arc<OAuth2Client>,
    #[inject]
    pending: Arc<PendingStore>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<GitHubStart, Response> for GitHubStartHandler {
    async fn handle(&mut self, _request: GitHubStart) -> ActionResult<Response> {
        let auth = self
            .client
            .authorization_url(uuid::Uuid::new_v4().to_string())?;
        self.pending
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(auth.state.clone(), auth.pkce_verifier);
        Ok(Box::new(
            ContentResult::text("")
                .status(302)
                .header("location", auth.url),
        ))
    }
}

#[derive(WebxRequestMeta)]
struct GitHubCallback {
    #[fromquery]
    code: Option<String>,
    #[fromquery]
    state: Option<String>,
}

#[get("/auth/github/callback")]
impl IRequest<Response> for GitHubCallback {}

#[derive(serde::Serialize)]
struct LoginResponse {
    token: String,
    provider: &'static str,
    subject: String,
    name: String,
    email: Option<String>,
}

#[derive(Inject)]
struct GitHubCallbackHandler {
    #[inject]
    client: Arc<OAuth2Client>,
    #[inject]
    pending: Arc<PendingStore>,
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<GitHubCallback, Response> for GitHubCallbackHandler {
    async fn handle(&mut self, request: GitHubCallback) -> ActionResult<Response> {
        let state = request
            .state
            .filter(|state| !state.is_empty())
            .ok_or_else(|| Error::unauthorized("missing OAuth state".to_string()))?;
        let verifier = self
            .pending
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&state)
            .ok_or_else(|| Error::unauthorized("unknown or reused OAuth state".to_string()))?;
        let code = request
            .code
            .filter(|code| !code.is_empty())
            .ok_or_else(|| Error::unauthorized("missing OAuth code".to_string()))?;

        let token = self
            .client
            .exchange_code(&code, verifier.as_deref())
            .await?;
        let user: OAuth2User = self.client.userinfo(&token).await?;
        let issued = self
            .tokens
            .issue(TokenClaims::new(&user.subject).with_roles(["user"]))
            .await?;
        Ok(Box::new(JsonResult::new(LoginResponse {
            token: issued.token,
            provider: "github",
            subject: user.subject,
            name: user.name,
            email: user.email,
        })))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let client_id =
        std::env::var("GITHUB_CLIENT_ID").map_err(|_| Error::di("GITHUB_CLIENT_ID is required"))?;
    let client_secret = std::env::var("GITHUB_CLIENT_SECRET")
        .map_err(|_| Error::di("GITHUB_CLIENT_SECRET is required"))?;
    let redirect = std::env::var("GITHUB_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:5000/auth/github/callback".to_string());
    let client = Arc::new(OAuth2Client::new(
        OAuth2Provider::github(),
        OAuth2Config::new(client_id, Some(client_secret), redirect),
    )?);
    let pending = Arc::new(PendingStore::default());

    Host::builder()
        .configure(|app| {
            app.useOptions(|options| {
                options.app.urls = vec!["http://localhost:5000".to_string()];
                options.jwt.secret = "dev-only-github-oauth-secret-change-me".to_string();
            });
        })
        .register(move |services| {
            services
                .try_add_singleton::<OAuth2Client>(move |_| Arc::clone(&client))
                .try_add_singleton::<PendingStore>(move |_| Arc::clone(&pending))
        })
        .add_authentication()
        .build()
        .run()
        .await
}
