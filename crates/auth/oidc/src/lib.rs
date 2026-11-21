//! OpenID Connect relying-party client for rust-webx.
//!
//! ```ignore
//! let http = std::sync::Arc::new(HttpOidcHttp::new("https://id.example.com", Duration::from_secs(3))?);
//! let oidc = OidcClient::discover(http, "https://id.example.com", "travel-web", ClientAuth::None).await?;
//! let request = oidc.authorization_url(AuthorizationRequest::new("https://app.example.com/cb").pkce_s256())?;
//! // store request.state / request.nonce / request.pkce_verifier in the browser session
//! let session = oidc
//!     .exchange_code(code, "https://app.example.com/cb", request.pkce_verifier.as_deref(), Some(&request.nonce))
//!     .await?;
//! let claims = session.to_token_claims();
//! ```
//!
//! Security defaults: HTTPS-only endpoints, S256 required, id_token `iss`/`aud`/
//! `nonce`/`exp`/`alg` validated, JWKS through the M2 resolver (cached,
//! single-flight, SSRF-guarded).

pub mod client;
pub mod discovery;
pub mod http;
pub mod pkce;

pub use client::{
    AuthorizationRequest, AuthorizationUrl, ClientAuth, OidcClient, OidcConfig, OidcSession,
};
pub use discovery::OidcDiscovery;
pub use http::{HttpOidcHttp, IOidcHttp, OIDC_MAX_BODY_BYTES};
pub use pkce::Pkce;
