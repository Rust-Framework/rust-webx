//! Token issuance port ([`ITokenIssuer`]).
//!
//! The framework only *declares* the port: signing needs a concrete algorithm
//! and key management, so the default JWT implementation lives in
//! `rust-webx-host` (`JwtTokenIssuer`). Applications
//! register their issuer in DI and call it from the login endpoint.

use crate::error::Result;
use std::collections::BTreeMap;
use std::time::Duration;

/// Claims a caller hands to an [`ITokenIssuer`].
///
/// Values are deliberately plain (`String` / `Vec<String>`) so the issuer does
/// not need to know the application's claim structs. Anything that is not a
/// first-class field — `tenant_id`, `name`, `email`, custom scopes — goes into
/// [`TokenClaims::extra`] and is embedded verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenClaims {
    /// Subject (`sub`) — the stable user identifier. Required.
    pub subject: String,

    /// Roles to embed (`roles` claim).
    pub roles: Vec<String>,

    /// Fine-grained permissions to embed (`permissions` claim).
    pub permissions: Vec<String>,

    /// Additional string claims, embedded verbatim (e.g. `tenant_id`).
    ///
    /// The issuer owns `sub` / `jti` / `exp` / `iat`: a key collision in `extra`
    /// must not be able to override those.
    pub extra: BTreeMap<String, String>,

    /// Requested lifetime. `None` means the issuer's configured default.
    pub expires_in: Option<Duration>,
}

impl TokenClaims {
    /// Claims for `subject` with no roles, permissions or extra claims.
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            ..Self::default()
        }
    }

    /// Set the `roles` claim.
    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Set the `permissions` claim.
    pub fn with_permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    /// Add one extra string claim.
    pub fn with_claim(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// Override the issuer's default lifetime.
    pub fn expires_in(mut self, lifetime: Duration) -> Self {
        self.expires_in = Some(lifetime);
        self
    }
}

/// A token minted by an [`ITokenIssuer`].
///
/// `jti` is what [`ISessionStore::revoke`](crate::security::ISessionStore::revoke)
/// stores: the login endpoint persists it (or hands it to the client for
/// revocation), and the authentication middleware rejects the token once the
/// `jti` is revoked — no need to wait for `expires_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    /// The serialized token (JWT compact form for the default issuer).
    pub token: String,

    /// Revocation identifier (`jti` claim) embedded in the token.
    pub jti: String,

    /// Expiry as a Unix timestamp in seconds.
    pub expires_at: u64,
}

/// Mint a token from [`TokenClaims`].
///
/// Implementations embed a unique [`IssuedToken::jti`] and an absolute
/// [`IssuedToken::expires_at`], so callers can revoke the token later without
/// decoding it again.
///
/// ```ignore
/// let issued = token_issuer.issue(
///     TokenClaims::new(&user.id)
///         .with_roles(user.roles.clone())
///         .with_claim("tenant_id", &user.tenant_id),
/// ).await?;
/// store.revoke(&issued.jti).await?; // logout
/// ```
#[async_trait::async_trait]
pub trait ITokenIssuer: Send + Sync {
    /// Sign `claims` into a token.
    ///
    /// Returns a typed [`Error`](crate::error::Error) for caller mistakes (for
    /// example an empty subject) and for signing failures; it must not panic.
    async fn issue(&self, claims: TokenClaims) -> Result<IssuedToken>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_claims_builder_sets_every_dimension() {
        let claims = TokenClaims::new("user-1")
            .with_roles(["admin", "user"])
            .with_permissions(["order.create"])
            .with_claim("tenant_id", "acme")
            .expires_in(Duration::from_secs(60));

        assert_eq!(claims.subject, "user-1");
        assert_eq!(claims.roles, vec!["admin", "user"]);
        assert_eq!(claims.permissions, vec!["order.create"]);
        assert_eq!(claims.extra.get("tenant_id").map(String::as_str), Some("acme"));
        assert_eq!(claims.expires_in, Some(Duration::from_secs(60)));
    }

    #[test]
    fn token_claims_default_lifetime_is_the_issuers_default() {
        assert_eq!(TokenClaims::new("user-1").expires_in, None);
    }
}
