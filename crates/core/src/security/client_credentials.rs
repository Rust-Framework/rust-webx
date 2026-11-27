//! OAuth 2.0 client-credentials port ([`IClientCredentials`]).
//!
//! The framework only *declares* the port: obtaining a machine token means
//! speaking to one specific IdP, so the implementation lives in
//! `rust-webx-oauth2` (`ClientCredentialsClient`). Application and adapter code
//! inject `Arc<dyn IClientCredentials>` and call [`IClientCredentials::token`];
//! the implementation owns the token cache, early refresh, single-flight and
//! fail-closed error mapping, so callers never see an empty or unauthenticated
//! result.
//!
//! ```ignore
//! #[async_trait::async_trait]
//! impl IBillingGateway for HttpBillingGateway {
//!     async fn charge(&self, order: &Order) -> Result<()> {
//!         let token = self.credentials.token(&["billing.write"]).await?;
//!         self.http
//!             .execute(
//!                 OutboundRequest::post("/charges")?
//!                     .header("authorization", format!("Bearer {}", token.access_token))
//!                     .json(order)?,
//!             )
//!             .await?;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! The token is a **machine** identity: it carries no user, no tenant and no
//! roles. Authorization for an end user is decided by the resource server from
//! the token's scopes, never by mixing a user token into a service call.

use std::fmt;

use crate::error::Result;

/// A machine token obtained through the OAuth 2.0 client-credentials grant.
///
/// `access_token` is a bearer credential: it is redacted from `Debug` so a
/// `tracing` field or an error context cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceToken {
    /// The bearer token to present to the resource server.
    pub access_token: String,

    /// Absolute expiry (`now + expires_in`) as a **Unix timestamp in seconds**.
    ///
    /// Deliberately seconds, like [`IssuedToken::expires_at_unix_secs`]. A
    /// caller that compares it with its own clock should keep a safety margin
    /// instead of using the last second.
    ///
    /// [`IssuedToken::expires_at_unix_secs`]: crate::security::IssuedToken::expires_at_unix_secs
    pub expires_at_unix_secs: u64,

    /// Space-separated scopes the token was requested for, as echoed by the
    /// token endpoint (or the requested set when it echoes none).
    pub scope: String,
}

impl fmt::Debug for ServiceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceToken")
            .field("access_token", &"<redacted>")
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .field("scope", &self.scope)
            .finish()
    }
}

/// Obtain a machine token for a service-to-service call.
///
/// Implementations cache per `(scope set, audience)` and refresh ahead of
/// expiry, so call sites may ask on every outbound request without paying for a
/// token round trip each time.
#[async_trait::async_trait]
pub trait IClientCredentials: Send + Sync {
    /// A token for `scope`.
    ///
    /// `scope` overrides the implementation's configured scopes; an empty slice
    /// means "no `scope` parameter" (the IdP's default scope set), not "the
    /// configured scopes". Returns a typed [`Error`](crate::error::Error) for a
    /// rejected client, a transient endpoint failure or an unusable response;
    /// it must never return an empty token.
    async fn token(&self, scope: &[&str]) -> Result<ServiceToken>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_access_token_is_redacted_from_debug() {
        let token = ServiceToken {
            access_token: "tok-secret".to_string(),
            expires_at_unix_secs: 1_700_000_000,
            scope: "billing.write".to_string(),
        };
        let debug = format!("{token:?}");
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("billing.write"));
        assert!(debug.contains("1700000000"));
        assert!(!debug.contains("tok-secret"), "leaked: {debug}");
    }
}
