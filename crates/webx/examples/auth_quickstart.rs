//! Compile-checked version of `docs/rust-webx/09-auth-security/quickstart.md`.
//!
//! ```text
//! cargo run -p rust-webx --example auth_quickstart
//! ```
//!
//! Demo only: the password hash is the well-known `admin123` Argon2id hash and
//! the JWT secret is dev-only. Production uses a real user store, a strong
//! secret (or `Jwt.SecretFile`) and `ILoginThrottle`.

use std::sync::Arc;

use webx::*;

/// Argon2id PHC for the demo password `admin123`.
const DEMO_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$oPXAx056HRCWKUdW9W+Q9g$B7eyRVgvnDQoT4OFohCyZQempaBVjTD54UQ6nqx3nWw";

#[derive(WebxRequestMeta, serde::Deserialize)]
struct LoginRequest {
    #[frombody]
    email: String,
    #[frombody]
    password: String,
}

#[derive(serde::Serialize)]
struct LoginResponse {
    token: String,
}

#[derive(Inject)]
struct LoginHandler {
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

#[post("/auth/login")]
impl IRequest<LoginResponse> for LoginRequest {}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<LoginRequest, LoginResponse> for LoginHandler {
    async fn handle(&mut self, request: LoginRequest) -> ActionResult<LoginResponse> {
        // Replace this with a user lookup; never reveal which field was wrong.
        if !self
            .hasher
            .verify(&request.password, DEMO_PASSWORD_HASH)
            .await?
        {
            return Err(Error::unauthorized("invalid credentials"));
        }
        let issued = self
            .tokens
            .issue(TokenClaims::new(&request.email).with_roles(["user"]))
            .await?;
        Ok(LoginResponse {
            token: issued.token,
        })
    }
}

#[derive(WebxRequestMeta)]
struct MeRequest {
    #[fromclaim]
    user: IdentityUser,
}

#[derive(serde::Serialize)]
struct MeResponse {
    id: String,
    tenant: Option<String>,
}

#[get("/me")]
impl IRequest<MeResponse> for MeRequest {}

#[derive(Default)]
struct MeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<MeRequest, MeResponse> for MeHandler {
    async fn handle(&mut self, request: MeRequest) -> ActionResult<MeResponse> {
        Ok(MeResponse {
            id: request.user.id.clone(),
            tenant: Some(request.user.tenant_id.clone()),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    Host::builder()
        .configure(|app| {
            app.useOptions(|options| {
                options.app.urls = vec!["http://127.0.0.1:5000".to_string()];
                options.jwt.secret = "dev-only-secret-change-me-at-least-32-bytes".to_string();
            });
        })
        .add_authentication()
        .build()
        .run()
        .await
}
