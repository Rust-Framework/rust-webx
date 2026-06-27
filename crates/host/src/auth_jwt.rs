//! JWT authentication module for the LRWF framework.
//!
//! Provides `JwtAuth` â€?an `IAuthenticationHandler` implementation that reads
//! a Bearer token from the `Authorization` header and validates it using
//! the `jsonwebtoken` crate.
//!
//! # Example
//!
//! ```ignore
//! use rust_webapp_host::auth_jwt::{JwtAuth, jwt_middleware};
//! use jsonwebtoken::{DecodingKey, Validation};
//!
//! let auth = JwtAuth::new(
//!     DecodingKey::from_secret(b"my-secret"),
//!     Validation::default(),
//! );
//! let middleware = jwt_middleware(Arc::new(auth));
//! ```

use jsonwebtoken::{decode, DecodingKey, Validation};
use rust_webapp_core::auth::{IAuthenticationHandler, IClaims};
use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

// ---------------------------------------------------------------------------
// JwtClaims â€?IClaims implementation backed by JWT payload
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
// JwtAuth â€?IAuthenticationHandler implementation
// ---------------------------------------------------------------------------

/// JWT-based authentication handler.
///
/// Reads the `Authorization: Bearer <token>` header, decodes and validates
/// the JWT, and returns the resulting claims.
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
        // Extract token synchronously â€?avoids holding &dyn IHttpContext
        // across any async boundary, keeping the future Send.
        let token = match ctx.request().header("authorization") {
            Some(h) => h.strip_prefix("Bearer ").map(|t| t.trim().to_string()),
            None => return Ok(None),
        };

        let token = match token {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(None),
        };

        let token_data = decode::<RawClaims>(&token, &self.decoding_key, &self.validation)
            .map_err(|e| rust_webapp_core::error::Error::Http(format!("JWT decode error: {}", e)))?;

        let claims: JwtClaims = token_data.claims.into();
        Ok(Some(Box::new(claims)))
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Authentication middleware that uses the given `IAuthenticationHandler`
/// to extract claims from the request and store them in the context.
struct AuthMiddleware {
    handler: Arc<dyn IAuthenticationHandler>,
}

#[async_trait::async_trait]
impl IMiddleware for AuthMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        if let Some(claims) = self.handler.authenticate(ctx).await? {
            // IHttpContext extends IClaimsExt, so set_claims is available directly.
            ctx.set_claims(claims);
        }
        Ok(())
    }
}

/// Create a JWT authentication middleware from an authentication handler.
///
/// Reads the Bearer token from `Authorization` header, validates it, and
/// stores the resulting claims in the HTTP context for downstream use.
pub fn jwt_middleware(handler: Arc<dyn IAuthenticationHandler>) -> impl IMiddleware {
    AuthMiddleware { handler }
}

// ---------------------------------------------------------------------------
// Global JWT encoding secret (for token creation in handlers)
// ---------------------------------------------------------------------------

static JWT_ENCODING_SECRET: OnceLock<String> = OnceLock::new();

/// Initialize the global JWT encoding secret from the configured secret.
/// This is called automatically by `.add_authentication()` on the `HostBuilder`,
/// but can also be called manually if needed.
///
/// # Panics
/// Panics if called more than once (it is intended to be set once at startup).
pub fn init_jwt_secret(secret: &str) {
    JWT_ENCODING_SECRET
        .set(secret.to_owned())
        .expect("init_jwt_secret has already been called");
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
