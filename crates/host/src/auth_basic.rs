//! HTTP Basic authentication scheme (RFC 7617).
//!
//! Basic sends `username:password` base64-encoded — **not encrypted**. Only
//! enable it behind TLS (or on a trusted loopback/sidecar path). It exists for
//! legacy integrations, health probes and simple service-to-service calls, not
//! for browser logins.
//!
//! The scheme only decodes and delegates. Credential storage, constant-time
//! comparison and account state belong to [`IBasicCredentialValidator`]; a
//! database-backed validator should reuse [`IPasswordHasher`](webx_core::security::IPasswordHasher).
//!
//! ```ignore
//! struct EnvValidator { user: String, password: String }
//!
//! #[async_trait::async_trait]
//! impl IBasicCredentialValidator for EnvValidator {
//!     async fn validate(&self, user: &str, password: &str)
//!         -> Result<Option<Box<dyn IClaims>>>
//!     {
//!         let ok = constant_time_eq(user.as_bytes(), self.user.as_bytes())
//!             & constant_time_eq(password.as_bytes(), self.password.as_bytes());
//!         Ok(ok.then(|| Box::new(BasicClaims::new(user)) as Box<dyn IClaims>))
//!     }
//! }
//!
//! Host::builder()
//!     .add_authentication_scheme(Arc::new(BasicScheme::new(Arc::new(EnvValidator { .. }))))
//!     .build();
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;

/// Stable scheme name used by diagnostics.
pub const BASIC_SCHEME_NAME: &str = "basic";

/// Verify a decoded username/password pair.
#[async_trait::async_trait]
pub trait IBasicCredentialValidator: Send + Sync {
    /// Return claims for a valid pair, `None` for invalid credentials.
    ///
    /// An `Err` is a backend failure (mapped to `5xx`), not a wrong password.
    async fn validate(&self, username: &str, password: &str) -> Result<Option<Box<dyn IClaims>>>;
}

/// Claims for a Basic-authenticated principal.
///
/// `subject` is the username; roles/permissions can be added by the validator
/// through a custom [`IClaims`] implementation when needed.
pub struct BasicClaims {
    username: String,
    raw: HashMap<String, String>,
}

impl BasicClaims {
    /// Claims for `username` with no roles/permissions.
    pub fn new(username: impl Into<String>) -> Self {
        let username = username.into();
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), username.clone());
        Self { username, raw }
    }
}

impl IClaims for BasicClaims {
    fn subject(&self) -> &str {
        &self.username
    }

    fn roles(&self) -> &[String] {
        &[]
    }

    fn permissions(&self) -> &[String] {
        &[]
    }

    fn claims(&self) -> &HashMap<String, String> {
        &self.raw
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(Self {
            username: self.username.clone(),
            raw: self.raw.clone(),
        })
    }
}

/// `Authorization: Basic …` scheme.
pub struct BasicScheme {
    validator: Arc<dyn IBasicCredentialValidator>,
}

impl BasicScheme {
    /// Build the scheme over a credential validator.
    pub fn new(validator: Arc<dyn IBasicCredentialValidator>) -> Self {
        Self { validator }
    }
}

#[async_trait::async_trait]
impl IAuthenticationScheme for BasicScheme {
    fn name(&self) -> &'static str {
        BASIC_SCHEME_NAME
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        ctx.request()
            .header("authorization")
            .and_then(basic_token)
            .is_some()
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        let Some(encoded) = ctx.request().header("authorization").and_then(basic_token) else {
            return Ok(None);
        };

        let decoded = decode_base64(encoded)
            .ok_or_else(|| Error::unauthorized("invalid basic credentials".to_string()))?;
        let decoded = String::from_utf8(decoded)
            .map_err(|_| Error::unauthorized("invalid basic credentials".to_string()))?;
        // RFC 7617: the username must not contain ':'; the password may.
        let (username, password) = decoded
            .split_once(':')
            .ok_or_else(|| Error::unauthorized("invalid basic credentials".to_string()))?;
        if username.is_empty() {
            return Err(Error::unauthorized("invalid basic credentials".to_string()));
        }

        match self.validator.validate(username, password).await? {
            Some(claims) => Ok(Some(claims)),
            None => Err(Error::unauthorized("invalid basic credentials".to_string())),
        }
    }
}

/// Extract the base64 payload from a case-insensitive `Basic` header.
fn basic_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    let engine = base64::engine::general_purpose::STANDARD;
    engine
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_header_is_case_insensitive_and_trims() {
        assert_eq!(basic_token("Basic dXNlcjpwYXNz"), Some("dXNlcjpwYXNz"));
        assert_eq!(basic_token("basic   dXNlcjpwYXNz  "), Some("dXNlcjpwYXNz"));
        assert_eq!(basic_token("Bearer dXNlcjpwYXNz"), None);
        assert_eq!(basic_token("Basic "), None);
        assert_eq!(basic_token("Basic"), None);
    }

    #[test]
    fn decodes_standard_and_unpadded_base64() {
        assert_eq!(
            decode_base64("dXNlcjpwYXNz").as_deref(),
            Some(b"user:pass".as_slice())
        );
        // "user:pas" needs one padding byte; the padded form must decode too.
        assert_eq!(
            decode_base64("dXNlcjpwYXM=").as_deref(),
            Some(b"user:pas".as_slice())
        );
        assert!(decode_base64("not base64!").is_none());
    }

    #[test]
    fn claims_expose_the_username_as_subject() {
        let claims = BasicClaims::new("alice");
        assert_eq!(claims.subject(), "alice");
        assert_eq!(
            claims.claims().get("sub").map(String::as_str),
            Some("alice")
        );
    }
}
