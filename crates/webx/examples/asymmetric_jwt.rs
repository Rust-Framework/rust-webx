//! Compile-checked demo of asymmetric JWT signing + JWKS publication (F1).
//!
//! One host does all three: it signs ES256 tokens with its own P-256 private
//! key, verifies them through the same key set (no shared HMAC secret) and
//! publishes the public half as a standard JWKS document. The JWKS endpoint
//! returns `JwtKeySet::jwks()`, a plain `Serialize` DTO — no hand-written JSON.
//!
//! ```text
//! cargo run -p rust-webx --example asymmetric_jwt
//! ```
//!
//! Then, in another shell:
//!
//! ```text
//! # 1. sign in -> {"token":"<jwt>"}
//! curl -s -X POST http://127.0.0.1:5000/auth/login \
//!   -H 'content-type: application/json' -d '{"name":"alice"}'
//!
//! # 2. call the protected endpoint with the token from step 1
//! TOKEN='<paste the token>'
//! curl -s http://127.0.0.1:5000/api/me -H "authorization: Bearer $TOKEN"
//!
//! # 3. the public key set any other resource server can verify with
//! curl -s http://127.0.0.1:5000/.well-known/jwks.json
//! ```
//!
//! **Demo only.** The signing key is generated in-process and written to a
//! `tempfile`, so it dies with the process: a restart produces a different key
//! pair under the same `kid`, and tokens from the previous run stop verifying.
//! Production provisions `Jwt.Signing.PrivateKeyFile` from a secret store
//! (`chmod 600`; `Jwt.SecretFile` covers the HS256 secret path) or an external
//! key manager — never generate a signing key at startup.

use std::io::Write;
use std::sync::Arc;

use webx::*;

/// `Jwt.Signing.KeyId`: the JWT header `kid` and the JWKS entry's `kid`.
const DEMO_KEY_ID: &str = "demo-2026-11";

// ---------------------------------------------------------------------------
// POST /auth/login — mint an ES256 token through the framework port
// ---------------------------------------------------------------------------

#[derive(WebxRequestMeta, serde::Deserialize)]
struct LoginRequest {
    #[frombody]
    name: String,
}

#[derive(serde::Serialize)]
struct LoginResponse {
    token: String,
}

#[derive(Inject)]
struct LoginHandler {
    /// `Jwt.Signing` makes the default issuer sign asymmetrically, so the
    /// handler never touches key material: it hands over claims, not a key.
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
}

#[post("/auth/login")]
impl IRequest<LoginResponse> for LoginRequest {}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<LoginRequest, LoginResponse> for LoginHandler {
    async fn handle(&mut self, request: LoginRequest) -> ActionResult<LoginResponse> {
        // Replace this with a real user lookup; `tenant_id` is a claim the
        // identity layer reads back on every request.
        let issued = self
            .tokens
            .issue(TokenClaims::new(&request.name).with_claim("tenant_id", "demo-tenant"))
            .await?;
        Ok(LoginResponse {
            token: issued.token,
        })
    }
}

// ---------------------------------------------------------------------------
// GET /.well-known/jwks.json — publish the public keys
// ---------------------------------------------------------------------------

#[derive(WebxRequestMeta)]
struct JwksRequest {}

#[derive(Inject)]
struct JwksHandler {
    /// Registered as a singleton by `Host::build()` when `Jwt.Signing` is set.
    #[inject]
    keys: Arc<JwtKeySet>,
}

#[get("/.well-known/jwks.json")]
impl IRequest<JwkSet> for JwksRequest {}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<JwksRequest, JwkSet> for JwksHandler {
    async fn handle(&mut self, _request: JwksRequest) -> ActionResult<JwkSet> {
        // `JwkSet` is `Serialize`, so an ordinary DTO response writes standard
        // JSON: {"keys":[{"kty":"EC","kid":"demo-2026-11","alg":"ES256",...}]}
        // Only public parameters are emitted; the private key never leaves the
        // host.
        Ok(self.keys.jwks())
    }
}

// ---------------------------------------------------------------------------
// GET /api/me — the token the host signed, verified by the same host
// ---------------------------------------------------------------------------

#[derive(WebxRequestMeta)]
struct MeRequest {
    #[fromclaim]
    user: IdentityUser,
}

#[derive(serde::Serialize)]
struct MeResponse {
    id: String,
    tenant: Option<String>,
    client_id: String,
}

#[get("/api/me")]
impl IRequest<MeResponse> for MeRequest {}

#[derive(Default)]
struct MeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<MeRequest, MeResponse> for MeHandler {
    async fn handle(&mut self, request: MeRequest) -> ActionResult<MeResponse> {
        let tenant = request.user.tenant().map(str::to_string);
        Ok(MeResponse {
            id: request.user.id,
            tenant,
            client_id: request.user.client_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// Generate a throwaway P-256 key pair and hand the host its private key PEM.
///
/// The returned `NamedTempFile` deletes the file when dropped, so `main` keeps
/// the guard alive for as long as the server runs.
fn demo_signing_key_file() -> Result<tempfile::NamedTempFile> {
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|error| Error::di(format!("demo P-256 key generation failed: {error}")))?;
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|error| Error::di(format!("demo signing key file failed: {error}")))?;
    file.write_all(pair.serialize_pem().as_bytes())
        .map_err(|error| Error::di(format!("writing the demo signing key failed: {error}")))?;
    file.flush()
        .map_err(|error| Error::di(format!("flushing the demo signing key failed: {error}")))?;
    Ok(file)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Kept alive until the process exits: dropping it deletes the demo key.
    let signing_key_file = demo_signing_key_file()?;
    let signing_key_path = signing_key_file
        .path()
        .to_str()
        .ok_or_else(|| Error::di("the demo signing key path is not valid UTF-8".to_string()))?
        .to_string();

    // `JwtSigningSection` is `#[non_exhaustive]`: start from `Default` and set
    // the public fields, exactly as an operator's appsettings.json would.
    let mut signing = JwtSigningSection::default();
    signing.algorithm = JwtAlgorithm::Es256;
    signing.key_id = DEMO_KEY_ID.to_string();
    signing.private_key_file = signing_key_path;

    Host::builder()
        .mode(AppMode::Development)
        .configure(move |app| {
            app.options(move |options| {
                options.app.urls = vec!["http://127.0.0.1:5000".to_string()];
                // The one algorithm this host signs and verifies with; the
                // HMAC `Jwt.Secret` path stays unused.
                options.jwt.algorithms = vec![JwtAlgorithm::Es256];
                options.jwt.signing = Some(signing);
            });
        })
        .use_authentication()
        .try_build()?
        .run()
        .await
}
