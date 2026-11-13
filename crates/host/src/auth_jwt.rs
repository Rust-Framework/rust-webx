//! JWT authentication module for the rust-webx framework.
//!
//! Provides `JwtAuth` —an `IAuthenticationHandler` implementation that reads
//! a Bearer token from the `Authorization` header and validates it using
//! the `jsonwebtoken` crate.
//!
//! # Issuance and revocation (WEBX-2)
//!
//! Minting and invalidation are ports, so application code does not need its
//! own signing or session code:
//!
//! * [`JwtTokenIssuer`] implements `ITokenIssuer` and is registered
//!   automatically by `HostBuilder::add_authentication()` from `Jwt.Secret`.
//!   A login handler injects `Arc<dyn ITokenIssuer>` and calls
//!   [`ITokenIssuer::issue`].
//! * `.use_session_store(store)` registers an `ISessionStore`; the auth
//!   middleware then rejects a token whose `jti` was revoked. Revocation is
//!   opt-in: without a store the middleware only verifies signatures.
//!
//! ```ignore
//! // login: issue and remember the jti (or give it to the client)
//! let issued = token_issuer.issue(TokenClaims::new(&user.id)).await?;
//! // logout: revoke it; the next request with that token is 401
//! session_store.revoke(&issued.jti).await?;
//! ```
//!
//! # Example
//!
//! ```ignore
//! use webx_host::auth_jwt::{JwtAuth, jwt_middleware};
//! use jsonwebtoken::{DecodingKey, Validation};
//!
//! let auth = JwtAuth::new(
//!     DecodingKey::from_secret(b"my-secret"),
//!     Validation::default(),
//! );
//! let middleware = jwt_middleware(Arc::new(auth));
//! ```

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use webx_core::auth::{IAuthenticationHandler, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::security::{ISessionStore, ITokenIssuer, IssuedToken, TokenClaims};

// ---------------------------------------------------------------------------
// JwtClaims —IClaims implementation backed by JWT payload
// ---------------------------------------------------------------------------

/// Claims extracted from a JWT token.
///
/// By default, looks for `sub`, `roles`, and `permissions` claims.
/// All other claims are accessible via `claims()` as stringified values.
pub struct JwtClaims {
    sub: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    /// Pre-flattened raw claims map for the `claims()` method.
    raw: HashMap<String, String>,
}

/// Intermediate deserialization target. After deserialization we convert
/// into `JwtClaims` which stores pre-computed data for `IClaims`.
#[derive(Debug, Deserialize, Serialize)]
struct RawClaims {
    #[serde(default)]
    sub: String,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

impl JwtClaims {
    /// Create empty claims with just a subject.
    pub fn new(subject: impl Into<String>) -> Self {
        let sub = subject.into();
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), sub.clone());
        Self {
            sub,
            roles: Vec::new(),
            permissions: Vec::new(),
            raw,
        }
    }
}

impl From<RawClaims> for JwtClaims {
    fn from(rc: RawClaims) -> Self {
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), rc.sub.clone());
        for (k, v) in &rc.extra {
            match v {
                serde_json::Value::String(s) => {
                    raw.insert(k.clone(), s.clone());
                }
                other => {
                    raw.insert(k.clone(), other.to_string());
                }
            }
        }
        Self {
            sub: rc.sub,
            roles: rc.roles,
            permissions: rc.permissions,
            raw,
        }
    }
}

impl IClaims for JwtClaims {
    fn subject(&self) -> &str {
        &self.sub
    }

    fn roles(&self) -> &[String] {
        &self.roles
    }

    fn permissions(&self) -> &[String] {
        &self.permissions
    }

    fn claims(&self) -> &HashMap<String, String> {
        &self.raw
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(Self {
            sub: self.sub.clone(),
            roles: self.roles.clone(),
            permissions: self.permissions.clone(),
            raw: self.raw.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// JwtAuth —IAuthenticationHandler implementation
// ---------------------------------------------------------------------------

/// JWT-based authentication handler.
///
/// Reads the `Authorization: Bearer <token>` header, decodes and validates
/// the JWT, and returns the resulting claims. Only a **valid** token produces
/// claims: a missing header, a non-`Bearer` scheme or a token that fails
/// validation all yield `Ok(None)` — the request simply continues as anonymous.
pub struct JwtAuth {
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtAuth {
    /// Create a new JWT authenticator.
    pub fn new(decoding_key: DecodingKey, validation: Validation) -> Self {
        Self {
            decoding_key,
            validation,
        }
    }
}

#[async_trait::async_trait]
impl IAuthenticationHandler for JwtAuth {
    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        // Extract token synchronously —avoids holding &dyn IHttpContext
        // across any async boundary, keeping the future Send.
        let header = match ctx.request().header("authorization") {
            Some(h) => h,
            None => return Ok(None),
        };

        // RFC 6750: scheme is case-insensitive
        let token = if header.len() >= 7 && header[..7].eq_ignore_ascii_case("Bearer ") {
            header[7..].trim().to_string()
        } else {
            return Ok(None);
        };

        if token.is_empty() {
            return Ok(None);
        }

        // A rejected token means "not authenticated", not "reject the request":
        // the request continues as anonymous and the endpoint decides. This is
        // what keeps a public endpoint (login, health, docs) reachable while a
        // client still holds an expired token, instead of dead-locking it with a
        // 401 on every call. Protected endpoints respond 401 further down the
        // pipeline (`#[authorize]` rules or a required `#[fromclaim]` identity),
        // through the single unauthorized writer (detail "authentication
        // required" + `WWW-Authenticate`).
        let token_data = match decode::<RawClaims>(&token, &self.decoding_key, &self.validation) {
            Ok(data) => data,
            Err(e) => {
                // The decoder's message is for operators, not for the client
                // (it names InvalidToken/ExpiredSignature/InvalidAlgorithm/...).
                tracing::debug!(reason = %e, "JWT rejected");
                return Ok(None);
            }
        };

        let claims: JwtClaims = token_data.claims.into();
        Ok(Some(Box::new(claims)))
    }
}

// ---------------------------------------------------------------------------
// JwtTokenIssuer —ITokenIssuer implementation (default token minting)
// ---------------------------------------------------------------------------

/// Default token lifetime used when [`TokenClaims::expires_in`] is `None`.
pub const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(3600);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// JWT token issuer — the default [`ITokenIssuer`] (HS256).
///
/// Mints a token carrying `sub`, `roles`, `permissions`, the caller's extra
/// claims, and the framework-managed `jti` / `iat` / `exp`. The `jti` is what
/// [`ISessionStore::revoke`] later invalidates, so a login endpoint only needs:
///
/// ```ignore
/// let issued = issuer.issue(TokenClaims::new(&user.id)).await?;
/// sessions.revoke(&issued.jti).await?; // logout
/// ```
///
/// `HostBuilder::add_authentication()` registers one from the configured
/// `Jwt.Secret`; register a custom [`ITokenIssuer`] in DI to replace it (for
/// example to use an asymmetric algorithm).
pub struct JwtTokenIssuer {
    encoding_key: EncodingKey,
    default_lifetime: Duration,
    issuer: Option<String>,
    audience: Option<String>,
}

impl JwtTokenIssuer {
    /// Build from a prepared [`EncodingKey`] (any algorithm `jsonwebtoken`
    /// supports; the decoder must be configured to match).
    pub fn new(encoding_key: EncodingKey, default_lifetime: Duration) -> Self {
        Self {
            encoding_key,
            default_lifetime,
            issuer: None,
            audience: None,
        }
    }

    /// Build an HS256 issuer from a shared secret — the same secret
    /// `JwtAuth` verifies with.
    pub fn from_secret(secret: impl AsRef<[u8]>, default_lifetime: Duration) -> Self {
        Self::new(EncodingKey::from_secret(secret.as_ref()), default_lifetime)
    }

    /// Set the `iss` claim.
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Set the `aud` claim.
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }
}

#[async_trait::async_trait]
impl ITokenIssuer for JwtTokenIssuer {
    async fn issue(&self, claims: TokenClaims) -> Result<IssuedToken> {
        let subject = claims.subject.trim();
        if subject.is_empty() {
            return Err(Error::Validation(
                "token subject must not be empty".to_string(),
            ));
        }

        let lifetime = claims.expires_in.unwrap_or(self.default_lifetime);
        let issued_at = unix_now();
        let expires_at = issued_at.saturating_add(lifetime.as_secs());
        let jti = uuid::Uuid::new_v4().to_string();

        // Extra claims first: reserved claims must win, otherwise a caller could
        // override `sub`/`exp`/`jti` through `TokenClaims::extra`.
        let mut payload = serde_json::Map::new();
        for (key, value) in &claims.extra {
            payload.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
        payload.insert("sub".into(), serde_json::json!(subject));
        payload.insert("roles".into(), serde_json::json!(claims.roles));
        payload.insert(
            "permissions".into(),
            serde_json::json!(claims.permissions),
        );
        payload.insert("jti".into(), serde_json::json!(jti));
        payload.insert("iat".into(), serde_json::json!(issued_at));
        payload.insert("exp".into(), serde_json::json!(expires_at));
        if let Some(issuer) = &self.issuer {
            payload.insert("iss".into(), serde_json::json!(issuer));
        }
        if let Some(audience) = &self.audience {
            payload.insert("aud".into(), serde_json::json!(audience));
        }

        let token = encode(
            &Header::default(),
            &serde_json::Value::Object(payload),
            &self.encoding_key,
        )
        .map_err(|error| Error::Internal(format!("JWT signing failed: {error}")))?;

        Ok(IssuedToken {
            token,
            jti,
            expires_at,
        })
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Authentication middleware: verify the token, then reject revoked tokens.
struct AuthMiddleware {
    handler: Arc<dyn IAuthenticationHandler>,
    /// `None` = signature verification only (1.1 behaviour).
    session_store: Option<Arc<dyn ISessionStore>>,
}

#[async_trait::async_trait]
impl IMiddleware for AuthMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if let Some(claims) = self.handler.authenticate(ctx).await? {
            if let Some(store) = &self.session_store {
                // A token without `jti` cannot be matched against a revocation
                // record; it keeps its `exp`-based lifetime (no re-reject).
                if let Some(jti) = claims.jti() {
                    if store.is_revoked(jti).await? {
                        // Revoked credentials are a credentials problem, not an
                        // anonymous request: fail here so the host writes its
                        // single 401 (problem+json + `WWW-Authenticate`).
                        return Err(Error::Unauthorized("token has been revoked".to_string()));
                    }
                }
            }
            // IHttpContext extends IClaimsExt, so set_claims is available directly.
            ctx.set_claims(claims);
        }
        Ok(ControlFlow::Continue(()))
    }
}

/// Create a JWT authentication middleware from an authentication handler.
///
/// Reads the Bearer token from `Authorization` header, validates it, and
/// stores the resulting claims in the HTTP context for downstream use.
pub fn jwt_middleware(handler: Arc<dyn IAuthenticationHandler>) -> impl IMiddleware {
    AuthMiddleware {
        handler,
        session_store: None,
    }
}

/// Create a JWT authentication middleware that also rejects revoked tokens.
///
/// After the signature check, the token's `jti` is looked up in `session_store`;
/// a revoked token is answered with the framework's 401 (problem+json +
/// `WWW-Authenticate`) before the endpoint runs. Tokens without a `jti` pass
/// through unchanged.
pub fn jwt_middleware_with_revocation(
    handler: Arc<dyn IAuthenticationHandler>,
    session_store: Arc<dyn ISessionStore>,
) -> impl IMiddleware {
    AuthMiddleware {
        handler,
        session_store: Some(session_store),
    }
}

// ---------------------------------------------------------------------------
// Global JWT encoding secret (for token creation in handlers)
// ---------------------------------------------------------------------------

static JWT_ENCODING_SECRET: OnceLock<String> = OnceLock::new();

/// Initialize the global JWT encoding secret from the configured secret.
/// This is called automatically by `.add_authentication()` on the `HostBuilder`,
/// but can also be called manually if needed.
///
/// Idempotent: subsequent calls are no-ops (the first secret wins).
pub fn init_jwt_secret(secret: &str) {
    let _ = JWT_ENCODING_SECRET.set(secret.to_owned());
}

/// Retrieve the global JWT encoding secret previously set via [`init_jwt_secret`].
///
/// # Panics
/// Panics if [`init_jwt_secret`] has not been called yet.
pub fn jwt_secret() -> &'static str {
    JWT_ENCODING_SECRET
        .get()
        .expect("init_jwt_secret must be called before jwt_secret")
}
