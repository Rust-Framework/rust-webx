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
//!   automatically by `HostBuilder::use_authentication()` from `Jwt.Secret`.
//!   A login handler injects `Arc<dyn ITokenIssuer>` and calls
//!   [`ITokenIssuer::issue`].
//! * `.use_session_store(store)` registers an `ISessionStore`; the auth
//!   middleware then rejects a token whose `jti` was revoked. Revocation is
//!   opt-in: without a store the middleware only verifies signatures.
//!
//! # Secret sources (XR-2)
//!
//! The signing secret comes from the first non-empty of `APP__Jwt__Secret` /
//! `JWT_SECRET`, `Jwt.Secret`, and `Jwt.SecretFile`. The last one is read by
//! [`apply_jwt_secret_file`] during host start-up — a missing, unreadable,
//! short or (on Unix) group/other-accessible file aborts start-up with a
//! readable error instead of signing tokens with a broken key.
//!
//! # Custom issuers and the resolved secret
//!
//! `Jwt.SecretFile` is resolved **before** application service registrations
//! run, so a custom [`ITokenIssuer`] can use the same key the host's `JwtAuth`
//! verifies with in either of two ways:
//!
//! * `HostBuilder::register_with_options(|svc, options| ...)` — `options` is the
//!   final [`AppOptions`]; build the issuer from `options.jwt.secret`.
//! * `HostBuilder::register(|svc| ...)` plus the [`JwtSecret`] singleton — the
//!   host registers the resolved secret in DI before user registrations, so a
//!   lazily-invoked factory can call `resolve_arc::<JwtSecret>()`.
//!
//! Both keep the `SecretFile`-only deployment working: the file is read before
//! the issuer is built, so an issuer never sees an empty key.
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

use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use webx_core::auth::{IAuthenticationHandler, IClaims};
use webx_core::config::{
    AppOptions, InvalidTokenBehavior, JwtAlgorithm, JwtJwksSection, JwtSection,
    MAX_JWT_CLOCK_SKEW_SECONDS, MAX_JWT_TOKEN_BYTES,
};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::security::{ISessionStore, ITokenIssuer, IssuedToken, TokenClaims};

use crate::auth_jwks::IJwtKeyResolver;

/// Default ceiling for an accepted compact JWT, in bytes.
///
/// Tokens are rejected before signature verification when they exceed this
/// size, so a hostile header cannot force a large allocation.
pub const DEFAULT_MAX_TOKEN_BYTES: usize = 8 * 1024;

/// Default clock-skew allowance for `exp`/`nbf`/`iat`, in seconds.
pub const DEFAULT_CLOCK_SKEW_SECONDS: u64 = 30;

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

/// Validated JWT verification settings.
///
/// Built from [`JwtSection`] by [`JwtValidation::from_section`] or from code
/// with [`JwtValidation::builder`]. Construction is **fail-closed**: an empty
/// algorithm list, an algorithm the host cannot verify, an empty issuer or
/// audience, an oversized clock skew, or a zero/oversized token budget is a
/// typed error rather than a silent default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtValidation {
    algorithms: Vec<JwtAlgorithm>,
    issuer: Option<String>,
    audience: Option<String>,
    leeway: Duration,
    max_token_bytes: usize,
    invalid_token_behavior: InvalidTokenBehavior,
    jwks: Option<JwtJwksSection>,
}

impl JwtValidation {
    /// Start building a validation configuration.
    pub fn builder() -> JwtValidationBuilder {
        JwtValidationBuilder::default()
    }

    /// Build from the merged `Jwt` configuration section.
    pub fn from_section(section: &JwtSection) -> Result<Self> {
        let mut builder = JwtValidationBuilder::default()
            .algorithms(section.algorithms.iter().copied())
            .leeway(Duration::from_secs(section.clock_skew_seconds))
            .max_token_bytes(section.max_token_bytes)
            .invalid_token_behavior(section.invalid_token_behavior)
            .jwks(section.jwks.clone());
        if let Some(issuer) = &section.issuer {
            builder = builder.require_issuer(issuer.clone());
        }
        if let Some(audience) = &section.audience {
            builder = builder.require_audience(audience.clone());
        }
        builder.build()
    }

    /// Accepted algorithms.
    pub fn algorithms(&self) -> &[JwtAlgorithm] {
        &self.algorithms
    }

    /// Required `iss`, when configured.
    pub fn issuer(&self) -> Option<&str> {
        self.issuer.as_deref()
    }

    /// Required `aud`, when configured.
    pub fn audience(&self) -> Option<&str> {
        self.audience.as_deref()
    }

    /// Clock-skew allowance.
    pub fn leeway(&self) -> Duration {
        self.leeway
    }

    /// Maximum accepted token size, in bytes.
    pub fn max_token_bytes(&self) -> usize {
        self.max_token_bytes
    }

    /// Invalid-token semantics.
    pub fn invalid_token_behavior(&self) -> InvalidTokenBehavior {
        self.invalid_token_behavior
    }

    /// Remote JWKS configuration, when asymmetric algorithms are enabled.
    pub fn jwks(&self) -> Option<&JwtJwksSection> {
        self.jwks.as_ref()
    }

    fn jsonwebtoken_validation(&self) -> Validation {
        // Base validation; the resolver path narrows `algorithms` to the token
        // header's algorithm after the key is resolved.
        let algorithms: Vec<Algorithm> = self
            .algorithms
            .iter()
            .map(|algorithm| jsonwebtoken_algorithm(*algorithm))
            .collect();
        let mut validation =
            Validation::new(algorithms.first().copied().unwrap_or(Algorithm::HS256));
        validation.algorithms = algorithms;
        validation.leeway = self.leeway.as_secs();
        // `nbf` is only checked when the claim is present.
        validation.validate_nbf = true;

        let mut required: HashSet<String> = HashSet::new();
        required.insert("exp".to_string());
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer]);
            required.insert("iss".to_string());
        }
        if let Some(audience) = &self.audience {
            validation.set_audience(&[audience]);
            required.insert("aud".to_string());
        }
        validation.required_spec_claims = required;
        validation
    }
}

/// Builder for [`JwtValidation`].
#[derive(Debug, Clone)]
pub struct JwtValidationBuilder {
    algorithms: Vec<JwtAlgorithm>,
    issuer: Option<String>,
    audience: Option<String>,
    leeway: Duration,
    max_token_bytes: usize,
    invalid_token_behavior: InvalidTokenBehavior,
    jwks: Option<JwtJwksSection>,
}

impl Default for JwtValidationBuilder {
    fn default() -> Self {
        Self {
            algorithms: vec![JwtAlgorithm::Hs256],
            issuer: None,
            audience: None,
            leeway: Duration::from_secs(DEFAULT_CLOCK_SKEW_SECONDS),
            max_token_bytes: DEFAULT_MAX_TOKEN_BYTES,
            invalid_token_behavior: InvalidTokenBehavior::Anonymous,
            jwks: None,
        }
    }
}

impl JwtValidationBuilder {
    /// Replace the accepted algorithm list. Empty is rejected by [`Self::build`].
    pub fn algorithms(mut self, algorithms: impl IntoIterator<Item = JwtAlgorithm>) -> Self {
        self.algorithms = algorithms.into_iter().collect();
        self
    }

    /// Require the `iss` claim to equal `issuer`.
    pub fn require_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Require the `aud` claim to contain `audience`.
    pub fn require_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// Clock-skew allowance (`exp`/`nbf`/`iat`).
    pub fn leeway(mut self, leeway: Duration) -> Self {
        self.leeway = leeway;
        self
    }

    /// Maximum accepted token size, in bytes.
    pub fn max_token_bytes(mut self, max_token_bytes: usize) -> Self {
        self.max_token_bytes = max_token_bytes;
        self
    }

    /// Invalid-token semantics (anonymous vs immediate `401`).
    pub fn invalid_token_behavior(mut self, behavior: InvalidTokenBehavior) -> Self {
        self.invalid_token_behavior = behavior;
        self
    }

    /// Remote JWKS source for asymmetric algorithms.
    pub fn jwks(mut self, jwks: Option<JwtJwksSection>) -> Self {
        self.jwks = jwks;
        self
    }

    /// Validate cross-field constraints and produce the configuration.
    pub fn build(self) -> Result<JwtValidation> {
        if self.algorithms.is_empty() {
            return Err(Error::di(
                "Jwt.Algorithms must not be empty; set at least [\"HS256\"]".to_string(),
            ));
        }
        if self
            .issuer
            .as_deref()
            .is_some_and(|issuer| issuer.trim().is_empty())
        {
            return Err(Error::di(
                "Jwt.Issuer must not be empty when configured".to_string(),
            ));
        }
        if self
            .audience
            .as_deref()
            .is_some_and(|audience| audience.trim().is_empty())
        {
            return Err(Error::di(
                "Jwt.Audience must not be empty when configured".to_string(),
            ));
        }
        let has_asymmetric = self
            .algorithms
            .iter()
            .any(|algorithm| *algorithm != JwtAlgorithm::Hs256);
        if has_asymmetric && self.jwks.is_none() {
            return Err(Error::di(
                "Jwt.Algorithms contains an asymmetric algorithm but Jwt.Jwks is not configured"
                    .to_string(),
            ));
        }
        if let Some(jwks) = &self.jwks {
            if !jwks.url.starts_with("https://") {
                return Err(Error::di(format!(
                    "Jwt.Jwks.Url must be https:// (got '{}')",
                    jwks.url
                )));
            }
            if jwks.cache_seconds == 0 || jwks.cache_seconds > 86_400 {
                return Err(Error::di(
                    "Jwt.Jwks.CacheSeconds must be between 1 and 86400".to_string(),
                ));
            }
            if !(100..=10_000).contains(&jwks.timeout_ms) {
                return Err(Error::di(
                    "Jwt.Jwks.TimeoutMs must be between 100 and 10000".to_string(),
                ));
            }
            if !(1..=256).contains(&jwks.max_keys) {
                return Err(Error::di(
                    "Jwt.Jwks.MaxKeys must be between 1 and 256".to_string(),
                ));
            }
        }

        let issuer = normalize_claim_value("Jwt.Issuer", self.issuer)?;
        let audience = normalize_claim_value("Jwt.Audience", self.audience)?;

        if self.leeway > Duration::from_secs(MAX_JWT_CLOCK_SKEW_SECONDS) {
            return Err(Error::di(format!(
                "Jwt.ClockSkewSeconds must be <= {MAX_JWT_CLOCK_SKEW_SECONDS} (got {})",
                self.leeway.as_secs()
            )));
        }
        if self.max_token_bytes == 0 || self.max_token_bytes > MAX_JWT_TOKEN_BYTES {
            return Err(Error::di(format!(
                "Jwt.MaxTokenBytes must be between 1 and {MAX_JWT_TOKEN_BYTES} (got {})",
                self.max_token_bytes
            )));
        }

        Ok(JwtValidation {
            algorithms: self.algorithms,
            issuer,
            audience,
            leeway: self.leeway,
            max_token_bytes: self.max_token_bytes,
            invalid_token_behavior: self.invalid_token_behavior,
            jwks: self.jwks,
        })
    }
}

fn jsonwebtoken_algorithm(algorithm: JwtAlgorithm) -> Algorithm {
    match algorithm {
        JwtAlgorithm::Hs256 => Algorithm::HS256,
        JwtAlgorithm::Rs256 => Algorithm::RS256,
        JwtAlgorithm::Es256 => Algorithm::ES256,
        JwtAlgorithm::Ps256 => Algorithm::PS256,
    }
}

fn normalize_claim_value(label: &str, value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::di(format!(
            "{label} must not be empty when configured; remove the key or set a value"
        )));
    }
    if trimmed.len() > 1024 {
        return Err(Error::di(format!("{label} must be at most 1024 bytes")));
    }
    Ok(Some(trimmed.to_string()))
}

/// JWT-based authentication handler.
///
/// Reads the `Authorization: Bearer <token>` header, decodes and validates
/// the JWT, and returns the resulting claims. A missing header, a non-`Bearer`
/// scheme or a token that fails validation is **anonymous by default**; the
/// endpoint then decides (public endpoints stay reachable, protected ones
/// answer `401`). Configure [`InvalidTokenBehavior::Reject`] for OAuth 2.0
/// resource-server semantics, where a presented-but-invalid token is a `401`
/// immediately.
pub struct JwtAuth {
    decoding_key: Option<DecodingKey>,
    key_resolver: Option<Arc<dyn IJwtKeyResolver>>,
    validation: Validation,
    max_token_bytes: usize,
    invalid_token_behavior: InvalidTokenBehavior,
}

// `JwtAuth` is shared across middleware invocations. The trait-object resolver
// owns its own synchronization (the JWKS cache is behind a lock), so a panic
// during authentication leaves no observable broken invariant in `JwtAuth`
// itself. Asserting unwind safety keeps the 2.1 auto-trait surface intact
// (a removed auto trait is a semver-major change for `catch_unwind` users).
impl std::panic::UnwindSafe for JwtAuth {}
impl std::panic::RefUnwindSafe for JwtAuth {}

impl JwtAuth {
    /// Create a JWT authenticator with the framework defaults: anonymous on
    /// invalid tokens, 8 KiB token ceiling, the supplied `Validation`.
    pub fn new(decoding_key: DecodingKey, validation: Validation) -> Self {
        Self {
            decoding_key: Some(decoding_key),
            key_resolver: None,
            validation,
            max_token_bytes: DEFAULT_MAX_TOKEN_BYTES,
            invalid_token_behavior: InvalidTokenBehavior::Anonymous,
        }
    }

    /// Build from a validated [`JwtValidation`] (issuer/audience/leeway/size/
    /// behavior all applied consistently).
    pub fn from_validation(decoding_key: DecodingKey, validation: &JwtValidation) -> Self {
        Self {
            decoding_key: Some(decoding_key),
            key_resolver: None,
            validation: validation.jsonwebtoken_validation(),
            max_token_bytes: validation.max_token_bytes,
            invalid_token_behavior: validation.invalid_token_behavior,
        }
    }

    /// Build from a validated [`JwtValidation`] plus a key resolver.
    ///
    /// `decoding_key` is the HS256 secret when the host accepts HS256 for
    /// self-issued tokens; asymmetric tokens are resolved per `kid`.
    pub fn from_validation_with_resolver(
        decoding_key: Option<DecodingKey>,
        validation: &JwtValidation,
        resolver: Arc<dyn IJwtKeyResolver>,
    ) -> Self {
        Self {
            decoding_key,
            key_resolver: Some(resolver),
            validation: validation.jsonwebtoken_validation(),
            max_token_bytes: validation.max_token_bytes,
            invalid_token_behavior: validation.invalid_token_behavior,
        }
    }

    /// Override the token-size ceiling.
    pub fn with_max_token_bytes(mut self, max_token_bytes: usize) -> Self {
        self.max_token_bytes = max_token_bytes;
        self
    }

    /// Override what an invalid token means.
    pub fn with_invalid_token_behavior(mut self, behavior: InvalidTokenBehavior) -> Self {
        self.invalid_token_behavior = behavior;
        self
    }

    fn reject(&self, reason: &str) -> Result<Option<Box<dyn IClaims>>> {
        // The decoder's message is for operators, not for the client.
        tracing::debug!(reason, "JWT rejected");
        match self.invalid_token_behavior {
            InvalidTokenBehavior::Anonymous => Ok(None),
            InvalidTokenBehavior::Reject => Err(Error::unauthorized("invalid token".to_string())),
        }
    }

    fn iat_is_plausible(&self, claims: &JwtClaims) -> bool {
        let Some(issued_at) = claims.raw.get("iat").and_then(|v| v.parse::<u64>().ok()) else {
            return true;
        };
        let now = unix_now();
        issued_at <= now.saturating_add(self.validation.leeway)
    }
}

#[async_trait::async_trait]
impl IAuthenticationHandler for JwtAuth {
    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        // Extract the token synchronously — avoids holding `&dyn IHttpContext`
        // across any async boundary, keeping the future `Send`.
        let header = match ctx.request().header("authorization") {
            Some(header) => header,
            None => return Ok(None),
        };

        // RFC 6750: the scheme is case-insensitive. `split_once` keeps the
        // slice on a char boundary even for a non-ASCII header value.
        let token = match header.split_once(' ') {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("Bearer") => rest.trim(),
            _ => return Ok(None),
        };
        if token.is_empty() {
            return Ok(None);
        }
        self.validate_token(token).await
    }
}

impl JwtAuth {
    /// Validate a token string (no request extraction).
    ///
    /// Used by [`JwtAuth::authenticate`] and by sender-constrained schemes
    /// (DPoP) that carry the access token under a different scheme name.
    pub async fn validate_token(&self, token: &str) -> Result<Option<Box<dyn IClaims>>> {
        if token.len() > self.max_token_bytes {
            return self.reject("token exceeds Jwt.MaxTokenBytes");
        }

        let decoded = if let Some(resolver) = &self.key_resolver {
            let header = match decode_header(token) {
                Ok(header) => header,
                Err(error) => return self.reject(&format!("invalid JWT header: {error}")),
            };
            if !self.validation.algorithms.contains(&header.alg) {
                return self.reject(&format!(
                    "JWT algorithm {:?} is not in Jwt.Algorithms",
                    header.alg
                ));
            }
            let mut validation = self.validation.clone();
            validation.algorithms = vec![header.alg];
            if header.alg == Algorithm::HS256 {
                let Some(key) = &self.decoding_key else {
                    return self.reject("HS256 token but no Jwt.Secret is configured");
                };
                decode::<RawClaims>(token, key, &validation)
            } else {
                let kid = header.kid.as_deref().unwrap_or("");
                let key = match resolver.resolve(kid, header.alg).await {
                    // A key-backend outage is not "anonymous": fail closed.
                    Ok(key) => key,
                    Err(error) if error.status_code() == 503 => return Err(error),
                    Err(error) => {
                        return self.reject(&format!("JWKS key resolution failed: {error}"))
                    }
                };
                decode::<RawClaims>(token, key.as_ref(), &validation)
            }
        } else {
            let Some(key) = &self.decoding_key else {
                return self.reject("no JWT verification key is configured");
            };
            decode::<RawClaims>(token, key, &self.validation)
        };

        let token_data = match decoded {
            Ok(data) => data,
            Err(error) => {
                return self.reject(&format!(
                    "JWT decode failed (invalid/expired/algorithm/issuer/audience): {error}"
                ))
            }
        };

        let claims: JwtClaims = token_data.claims.into();
        // A signed token without a subject must never become an authenticated
        // identity: `IdentityUser::id` would be empty and tenant/ownership
        // checks would operate on a phantom principal.
        if claims.subject().trim().is_empty() {
            return self.reject("JWT has an empty `sub`");
        }
        if !self.iat_is_plausible(&claims) {
            return self.reject("JWT `iat` is in the future");
        }
        Ok(Some(Box::new(claims)))
    }
}

// ---------------------------------------------------------------------------
// JwtTokenIssuer —ITokenIssuer implementation (default token minting)
// ---------------------------------------------------------------------------

/// Default token lifetime used by [`JwtTokenIssuer::from_secret`] when the
/// caller does not pass one explicitly.
///
/// The host does **not** use this constant: `add_authentication()` builds the
/// issuer from the `Jwt.ExpirationMinutes` appsettings key (default 60 minutes).
pub const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(3600);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// JWT token issuer — the default [`ITokenIssuer`].
///
/// **HS256 only.** The token is signed with `HS256` and verified by this
/// crate's `JwtAuth`, whose `Validation::default()` also expects `HS256`, so
/// issuer and verifier agree out of the box. Asymmetric algorithms (RSA/EC)
/// are not supported by this type: using one requires *both* a custom
/// [`ITokenIssuer`] and a custom [`IAuthenticationHandler`] (the host's
/// `jwt_middleware` always verifies with `Validation::default()`).
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
/// `HostBuilder::use_authentication()` registers one from the configured
/// `Jwt.Secret` / `Jwt.ExpirationMinutes`; register a custom [`ITokenIssuer`] in
/// DI to replace it. `HostBuilder::register_with_options` hands the closure the
/// final `AppOptions` (with `Jwt.SecretFile` already resolved), so a custom
/// issuer can sign with the same key the host verifies.
pub struct JwtTokenIssuer {
    encoding_key: EncodingKey,
    default_lifetime: Duration,
    issuer: Option<String>,
    audience: Option<String>,
}

impl JwtTokenIssuer {
    /// Build an HS256 issuer from a shared secret — the same secret
    /// `JwtAuth` verifies with.
    ///
    /// `default_lifetime` applies when [`TokenClaims::expires_in`] is `None`;
    /// `DEFAULT_TOKEN_LIFETIME` (1 hour) is the framework default.
    pub fn from_secret(secret: impl AsRef<[u8]>, default_lifetime: Duration) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_ref()),
            default_lifetime,
            issuer: None,
            audience: None,
        }
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
            return Err(Error::validation(
                "token subject must not be empty".to_string(),
            ));
        }

        let lifetime = claims.expires_in.unwrap_or(self.default_lifetime);
        let issued_at = unix_now();
        let expires_at_unix_secs = issued_at.saturating_add(lifetime.as_secs());
        let jti = uuid::Uuid::new_v4().to_string();

        // Extra claims first: reserved claims must win, otherwise a caller could
        // override `sub`/`exp`/`jti` through `TokenClaims::extra`.
        let mut payload = serde_json::Map::new();
        for (key, value) in &claims.extra {
            payload.insert(key.clone(), serde_json::Value::String(value.clone()));
        }
        payload.insert("sub".into(), serde_json::json!(subject));
        payload.insert("roles".into(), serde_json::json!(claims.roles));
        payload.insert("permissions".into(), serde_json::json!(claims.permissions));
        payload.insert("jti".into(), serde_json::json!(jti));
        payload.insert("iat".into(), serde_json::json!(issued_at));
        payload.insert("exp".into(), serde_json::json!(expires_at_unix_secs));
        if let Some(issuer) = &self.issuer {
            payload.insert("iss".into(), serde_json::json!(issuer));
        }
        if let Some(audience) = &self.audience {
            payload.insert("aud".into(), serde_json::json!(audience));
        }

        // `Header::default()` is `HS256` — the only algorithm this issuer
        // promises, and the one `JwtAuth` verifies.
        let token = encode(
            &Header::default(),
            &serde_json::Value::Object(payload),
            &self.encoding_key,
        )
        .map_err(|error| Error::internal(format!("JWT signing failed: {error}")))?;

        Ok(IssuedToken {
            token,
            jti,
            expires_at_unix_secs,
        })
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Authenticate with a handler and apply the optional revocation check.
///
/// Shared by the standalone JWT middleware and the scheme chain
/// ([`crate::auth::JwtScheme`]) so both paths reject the same revoked tokens.
pub(crate) async fn authenticate_with_revocation(
    handler: &dyn IAuthenticationHandler,
    session_store: Option<&Arc<dyn ISessionStore>>,
    ctx: &mut dyn IHttpContext,
) -> Result<Option<Box<dyn IClaims>>> {
    let Some(claims) = handler.authenticate(ctx).await? else {
        return Ok(None);
    };

    if let Some(store) = session_store {
        // A token without `jti` cannot be matched against a revocation record;
        // it keeps its `exp`-based lifetime (no re-reject).
        if let Some(jti) = claims.jti() {
            if store.is_revoked(jti).await? {
                // Revoked credentials are a credentials problem, not an
                // anonymous request: fail here so the host writes its single
                // 401 (problem+json + `WWW-Authenticate`).
                return Err(Error::unauthorized("token has been revoked".to_string()));
            }
        }
    }

    Ok(Some(claims))
}

/// Authentication middleware: verify the token, then reject revoked tokens.
struct AuthMiddleware {
    handler: Arc<dyn IAuthenticationHandler>,
    /// `None` = signature verification only (1.1 behaviour).
    session_store: Option<Arc<dyn ISessionStore>>,
}

#[async_trait::async_trait]
impl IMiddleware for AuthMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if let Some(claims) =
            authenticate_with_revocation(&*self.handler, self.session_store.as_ref(), ctx).await?
        {
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
///
/// **Advanced entry point** — only for a hand-assembled middleware pipeline.
/// A regular host enables revocation with
/// [`HostBuilder::use_session_store`](crate::server::HostBuilder::use_session_store),
/// which registers the store and wires this middleware itself.
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

/// Deprecated 2.5 compatibility shim: initialize the global JWT encoding
/// secret.
///
/// `.use_authentication()` still populates this for applications that have not
/// migrated yet, but new code must inject `Arc<dyn ITokenIssuer>` and call
/// `issue(TokenClaims)` instead. The global will be removed in the next major.
/// (No `#[deprecated]` attribute in 2.x: adding one would break downstream
/// `-D warnings` builds in a minor release.)
pub fn init_jwt_secret(secret: &str) {
    let _ = JWT_ENCODING_SECRET.set(secret.to_owned());
}

/// Deprecated 2.5 compatibility shim: retrieve the global JWT encoding secret.
///
/// # Panics
/// Panics if the global was never initialized.
pub fn jwt_secret() -> &'static str {
    JWT_ENCODING_SECRET
        .get()
        .expect("init_jwt_secret must be called before jwt_secret")
}

// ---------------------------------------------------------------------------
// Jwt.SecretFile resolution (XR-2)
// ---------------------------------------------------------------------------

/// Minimum accepted length, in bytes, of a JWT signing secret read from a file.
///
/// HS256 keys shorter than 256 bits are brute-forceable, so a too-short file is
/// a startup error rather than a warning.
pub const MIN_JWT_SECRET_LEN: usize = 32;

/// The final JWT signing secret, registered in DI by the host.
///
/// `Jwt.SecretFile` is resolved before the application's service registrations
/// run, and the resolved value is registered as this singleton. A custom
/// [`ITokenIssuer`] factory can therefore lazily obtain exactly the key
/// `JwtAuth` verifies with:
///
/// ```ignore
/// svc.try_add_singleton::<dyn ITokenIssuer>(|resolver| {
///     let secret = resolver.resolve_arc::<JwtSecret>()?;
///     Arc::new(JwtTokenIssuer::from_secret(secret.as_str(), lifetime))
/// });
/// ```
///
/// The type is only registered when a non-empty secret was resolved: resolving
/// it is a start-up error rather than a silent empty-key signature.
#[derive(Clone)]
pub struct JwtSecret(Arc<str>);

impl JwtSecret {
    /// Wrap an already-resolved secret.
    pub fn new(secret: impl Into<Arc<str>>) -> Self {
        Self(secret.into())
    }

    /// Borrow the secret bytes as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Resolve `Jwt.SecretFile` into `Jwt.Secret`, failing fast on a bad file.
///
/// This is the third source in the documented priority chain — the first
/// non-empty one wins:
///
/// 1. `APP__Jwt__Secret` / `JWT_SECRET` — applied over the JSON by
///    `webx_core::config::load_appsettings`, so they are already in `Secret`;
/// 2. `Jwt.Secret` — an explicit value in `appsettings.json` or a
///    `HostBuilder` `useOptions` override;
/// 3. `Jwt.SecretFile` — the path configured here, read from disk.
///
/// The host calls this once while preparing itself (before any application
/// service registration, the default `ITokenIssuer` and the authentication
/// middleware are built), registers the resolved value as [`JwtSecret`] in DI,
/// and every later reader of `options.jwt.secret` — issuer, verifier,
/// `init_jwt_secret`, the Production weak-secret guard — sees the resolved
/// value.
///
/// # Behaviour
///
/// * No-op (`Ok(())`) when a secret is already configured or `SecretFile` is
///   unset/blank, so existing appsettings are unaffected.
/// * The file's content is trimmed; its surrounding whitespace (e.g. the
///   trailing newline `echo` writes) is not part of the key.
/// * Fails with [`Error::di`] — a host startup error, never a panic — when the
///   file is missing/unreadable, empty, shorter than [`MIN_JWT_SECRET_LEN`]
///   bytes, or (on Unix) accessible by group/other. `Error::di` is what the
///   existing Production configuration guards return; the message names the
///   path and the fix so an operator can act on it.
/// * **Windows:** the ACL is *not* validated — the POSIX mode model does not
///   exist there. Restrict the file to the service account (and
///   `Administrators`) yourself. DPAPI-wrapped secrets are a future item, not
///   part of this release.
///
/// # Example
///
/// ```
/// use webx_core::config::AppOptions;
/// use webx_host::auth_jwt::apply_jwt_secret_file;
///
/// let path = std::env::temp_dir().join(format!(
///     "webx-jwt-secret-{}-{}.key",
///     std::process::id(),
///     std::time::SystemTime::now()
///         .duration_since(std::time::UNIX_EPOCH)
///         .unwrap()
///         .as_nanos()
/// ));
/// # #[cfg(unix)]
/// # {
/// # use std::os::unix::fs::PermissionsExt;
/// # std::fs::write(&path, b"test-only-secret-material-32-bytes\n").unwrap();
/// # std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
/// # }
/// # #[cfg(not(unix))]
/// # std::fs::write(&path, b"test-only-secret-material-32-bytes\n").unwrap();
///
/// let mut options = AppOptions::default();
/// options.jwt.secret_file = Some(path.to_string_lossy().into_owned());
/// apply_jwt_secret_file(&mut options).unwrap();
/// assert_eq!(options.jwt.secret, "test-only-secret-material-32-bytes");
///
/// # std::fs::remove_file(&path).ok();
/// ```
pub fn apply_jwt_secret_file(options: &mut AppOptions) -> Result<()> {
    if !options.jwt.secret.trim().is_empty() {
        if options.jwt.secret_file.is_some() {
            // Deliberate, documented precedence — `Jwt.Secret` wins.
            tracing::debug!("[Auth] Jwt.Secret is set; ignoring Jwt.SecretFile");
        }
        return Ok(());
    }

    let Some(path) = options
        .jwt
        .secret_file
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return Ok(());
    };

    let secret = read_jwt_secret_file(Path::new(path))?;
    tracing::info!(
        "[Auth] JWT signing secret loaded from '{}' ({} bytes)",
        path,
        secret.len()
    );
    options.jwt.secret = secret;
    Ok(())
}

fn read_jwt_secret_file(path: &Path) -> Result<String> {
    verify_secret_file_permissions(path)?;

    let content = std::fs::read_to_string(path).map_err(|error| {
        Error::di(format!(
            "JWT secret file '{}' could not be read: {error}. \
             Provision a readable file (at least {MIN_JWT_SECRET_LEN} bytes, Unix mode 0600), \
             or set Jwt.Secret / APP__Jwt__Secret / JWT_SECRET.",
            path.display()
        ))
    })?;
    let secret = content.trim();
    if secret.is_empty() {
        return Err(Error::di(format!(
            "JWT secret file '{}' is empty; it must contain at least {MIN_JWT_SECRET_LEN} bytes of secret material.",
            path.display()
        )));
    }
    if secret.len() < MIN_JWT_SECRET_LEN {
        return Err(Error::di(format!(
            "JWT secret file '{}' contains only {} bytes; at least {MIN_JWT_SECRET_LEN} bytes are required.",
            path.display(),
            secret.len()
        )));
    }
    Ok(secret.to_string())
}

/// Reject a secret file that group/other can read or write (Unix only).
///
/// The POSIX analog of "the key file is private": anything but mode `0600`-or-
/// stricter (e.g. `0400`, `0700` for the owner) is refused. On Windows the ACL
/// is not inspected — start-up trusts the operator there.
#[cfg(unix)]
fn verify_secret_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|error| {
        Error::di(format!(
            "JWT secret file '{}' could not be inspected: {error}. \
             Provision it (at least {MIN_JWT_SECRET_LEN} bytes, Unix mode 0600), \
             or set Jwt.Secret / APP__Jwt__Secret / JWT_SECRET.",
            path.display()
        ))
    })?;
    let mode = metadata.permissions().mode();
    // 0o077 covers group/other read, write and execute bits.
    if mode & 0o077 != 0 {
        return Err(Error::di(format!(
            "JWT secret file '{}' has mode {mode:04o}; group/other access must be removed (`chmod 600`).",
            path.display()
        )));
    }
    Ok(())
}

/// Windows (and other non-Unix targets): no permission model to inspect.
#[cfg(not(unix))]
fn verify_secret_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
