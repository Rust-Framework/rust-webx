//! OAuth 2.0 social-login client for rust-webx.
//!
//! This crate is deliberately **not** an OIDC client: providers such as GitHub
//! issue an access token and expose a REST profile, not an `id_token`. Use
//! `rust-webx-oidc` when the provider supports OpenID Connect.
//!
//! The request-side primitives are:
//!
//! ```rust,ignore
//! let provider = OAuth2Provider::github();
//! let config = OAuth2Config::new(client_id, Some(client_secret), redirect_uri);
//! let client = OAuth2Client::new(provider, config)?;
//!
//! let auth = client.authorization_url("state-from-session")?;
//! // persist auth.state + auth.pkce_verifier, redirect the browser to auth.url
//!
//! let token = client.exchange_code(code, verifier.as_deref()).await?;
//! let user = client.userinfo(&token).await?;
//! let identity = user.into_identity();
//! ```
//!
//! Presets cover GitHub, Google, Microsoft, GitLab, Gitee, DingTalk, Feishu
//! and WeChat. Any other provider can be described with
//! [`OAuth2Provider::custom`] plus the builder methods.
//!
//! Service-to-service calls do not use a user token: [`client_credentials`]
//! implements the OAuth 2.0 `client_credentials` grant (RFC 6749 §4.4) with a
//! keyed token cache, early refresh and single-flight, and fails closed.

pub mod alipay;
pub mod client;
pub mod client_credentials;
pub mod provider;

pub use alipay::{AlipayClient, AlipayConfig, AlipayToken};

pub use client::{OAuth2Authorization, OAuth2Client, OAuth2Config, OAuth2Token, OAuth2User};
pub use client_credentials::{
    ClientCredentialsClient, ClientCredentialsConfig, ClientSecretAuth, ServiceToken,
};
pub use provider::{
    ClientAuth, OAuth2Provider, TokenFields, TokenFormat, TokenRequest, UserInfoAuth,
    UserInfoRequest,
};
