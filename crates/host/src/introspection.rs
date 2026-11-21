//! OAuth 2.0 token introspection (RFC 7662) and revocation (RFC 7009)
//! primitives.
//!
//! The endpoints themselves are ordinary rust-webx handlers; this module owns
//! the protocol semantics so every resource server answers the same way:
//!
//! * introspection never leaks why a token is inactive — it returns
//!   `{"active": false}` for invalid/expired/revoked tokens, and only a store
//!   outage becomes a 503;
//! * revocation always returns success for unknown tokens (RFC 7009) and
//!   revokes both JWT `jti`s and refresh-token families;
//! * the introspection client is authenticated with HTTP Basic, compared in
//!   constant time.

use std::sync::Arc;

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;
use webx_core::error::{Error, Result};
use webx_core::security::{constant_time_eq, ISessionStore};

use crate::auth_jwt::JwtAuth;
use crate::refresh::RefreshTokenService;

/// RFC 7662 introspection response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntrospectionResponse {
    /// Whether the token is currently usable.
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
}

impl IntrospectionResponse {
    fn inactive() -> Self {
        Self {
            active: false,
            scope: None,
            client_id: None,
            username: None,
            token_type: None,
            exp: None,
            iat: None,
            nbf: None,
            sub: None,
            aud: None,
            iss: None,
            jti: None,
        }
    }
}

/// Validates access tokens and reports their RFC 7662 metadata.
pub struct TokenIntrospector {
    validator: Arc<JwtAuth>,
    sessions: Option<Arc<dyn ISessionStore>>,
}

impl TokenIntrospector {
    /// Build with the same verifier the resource server uses.
    pub fn new(validator: Arc<JwtAuth>) -> Self {
        Self {
            validator,
            sessions: None,
        }
    }

    /// Also treat revoked `jti`s as inactive.
    pub fn with_session_store(mut self, sessions: Arc<dyn ISessionStore>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Introspect one token.
    ///
    /// Invalid/expired/revoked tokens produce `active: false`; a revocation
    /// store outage is a 503 (never silently "inactive").
    pub async fn introspect(&self, token: &str) -> Result<IntrospectionResponse> {
        let claims = match self.validator.validate_token(token).await {
            Ok(Some(claims)) => claims,
            Ok(None) => return Ok(IntrospectionResponse::inactive()),
            Err(error) if error.status_code() == 503 => return Err(error),
            Err(_) => return Ok(IntrospectionResponse::inactive()),
        };
        let raw = claims.claims();
        if let Some(store) = &self.sessions {
            if let Some(jti) = claims.jti() {
                if store.is_revoked(jti).await? {
                    return Ok(IntrospectionResponse::inactive());
                }
            }
        }

        Ok(IntrospectionResponse {
            active: true,
            scope: raw_string(raw, "scope"),
            client_id: raw_string(raw, "client_id"),
            username: raw_string(raw, "preferred_username").or_else(|| raw_string(raw, "name")),
            token_type: Some("Bearer".to_string()),
            exp: raw_u64(raw, "exp"),
            iat: raw_u64(raw, "iat"),
            nbf: raw_u64(raw, "nbf"),
            sub: Some(claims.subject().to_string()),
            aud: raw_aud(raw),
            iss: raw_string(raw, "iss"),
            jti: raw_string(raw, "jti"),
        })
    }
}

/// RFC 7009 token revocation for JWTs and refresh tokens.
pub struct TokenRevoker {
    validator: Arc<JwtAuth>,
    sessions: Arc<dyn ISessionStore>,
    refresh: Option<Arc<RefreshTokenService>>,
}

impl TokenRevoker {
    /// Build with the verifier and session store used by authentication.
    pub fn new(validator: Arc<JwtAuth>, sessions: Arc<dyn ISessionStore>) -> Self {
        Self {
            validator,
            sessions,
            refresh: None,
        }
    }

    /// Also revoke opaque refresh-token families.
    pub fn with_refresh(mut self, refresh: Arc<RefreshTokenService>) -> Self {
        self.refresh = Some(refresh);
        self
    }

    /// Revoke a token; unknown/invalid tokens are a successful no-op.
    ///
    /// A store outage is returned as 503 so the caller can fail closed.
    pub async fn revoke(&self, token: &str) -> Result<()> {
        // JWT compact serialization has exactly two dots; refresh tokens have
        // one. Treat anything else as unknown.
        if token.matches('.').count() == 2 {
            match self.validator.validate_token(token).await {
                Ok(Some(claims)) => {
                    let raw = claims.claims();
                    if let (Some(jti), Some(exp)) =
                        (claims.jti().map(ToString::to_string), raw_u64(raw, "exp"))
                    {
                        self.sessions.revoke_until(&jti, exp).await?;
                    }
                }
                Ok(None) => {}
                Err(error) if error.status_code() == 503 => return Err(error),
                Err(_) => {}
            }
            return Ok(());
        }

        if let Some(refresh) = &self.refresh {
            // The service verifies the secret before revoking the family;
            // unknown/malformed tokens are ignored per RFC 7009.
            let _ = refresh.revoke_token(token).await;
        }
        Ok(())
    }
}

/// HTTP Basic client authentication for the introspection endpoint.
pub struct IntrospectionBasicAuth {
    client_id: String,
    client_secret: String,
}

impl IntrospectionBasicAuth {
    /// Build from the registered introspection client credentials.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Result<Self> {
        let client_id = client_id.into();
        let client_secret = client_secret.into();
        if client_id.trim().is_empty() || client_secret.is_empty() {
            return Err(Error::Di(
                "introspection client id/secret must not be empty".to_string(),
            ));
        }
        Ok(Self {
            client_id,
            client_secret,
        })
    }

    /// Check an `Authorization` header; compares id and secret in constant time.
    pub fn authorize(&self, authorization: Option<&str>) -> bool {
        let Some(value) = authorization else {
            return false;
        };
        let Some((scheme, encoded)) = value.split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("Basic") {
            return false;
        }
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
            return false;
        };
        let Ok(decoded) = String::from_utf8(decoded) else {
            return false;
        };
        let Some((client_id, client_secret)) = decoded.split_once(':') else {
            return false;
        };
        let id_matches = constant_time_eq(client_id.as_bytes(), self.client_id.as_bytes());
        let secret_matches =
            constant_time_eq(client_secret.as_bytes(), self.client_secret.as_bytes());
        id_matches & secret_matches
    }
}

impl std::fmt::Debug for IntrospectionBasicAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntrospectionBasicAuth")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

fn raw_value(raw: &std::collections::HashMap<String, String>, key: &str) -> Option<Value> {
    serde_json::from_str(raw.get(key)?).ok()
}

fn raw_string(raw: &std::collections::HashMap<String, String>, key: &str) -> Option<String> {
    let text = raw.get(key)?;
    match serde_json::from_str::<Value>(text) {
        Ok(Value::String(value)) => Some(value),
        Ok(_) => None,
        // `JwtClaims::claims()` stores string claims bare (not JSON-quoted).
        Err(_) => Some(text.clone()),
    }
}

fn raw_aud(raw: &std::collections::HashMap<String, String>) -> Option<Value> {
    let text = raw.get("aud")?;
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Some(value),
        Err(_) => Some(Value::String(text.clone())),
    }
}

fn raw_u64(raw: &std::collections::HashMap<String, String>, key: &str) -> Option<u64> {
    raw_value(raw, key)?.as_u64()
}
