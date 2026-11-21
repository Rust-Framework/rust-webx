//! OpenID Connect discovery document (`.well-known/openid-configuration`).

use serde::Deserialize;
use webx_core::error::{Error, Result};

use crate::http::IOidcHttp;

/// The subset of the discovery document the relying party uses.
#[derive(Debug, Clone, Deserialize)]
pub struct OidcDiscovery {
    /// Canonical issuer identifier.
    pub issuer: String,
    /// Authorization endpoint.
    pub authorization_endpoint: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// JWKS endpoint used for id_token signature validation.
    pub jwks_uri: String,
    /// Optional userinfo endpoint.
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    /// Optional RP-initiated logout endpoint.
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
    /// PKCE methods; S256 must be present.
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    /// id_token signing algorithms.
    #[serde(default)]
    pub id_token_signing_alg_values_supported: Vec<String>,
    /// Supported response types; `code` must be present.
    #[serde(default)]
    pub response_types_supported: Vec<String>,
    /// Supported scopes (informational).
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

impl OidcDiscovery {
    /// Fetch and validate the discovery document for `issuer`.
    pub async fn fetch(http: &dyn IOidcHttp, issuer: &str) -> Result<Self> {
        Self::fetch_with(http, issuer, true).await
    }

    /// Fetch with an explicit PKCE-advertisement policy.
    ///
    /// Some providers (notably Microsoft multi-tenant discovery) support PKCE
    /// but omit `code_challenge_methods_supported`. Passing `false` accepts an
    /// empty list while still rejecting a provider that explicitly advertises
    /// only `plain`.
    pub async fn fetch_with(
        http: &dyn IOidcHttp,
        issuer: &str,
        require_pkce_advertised: bool,
    ) -> Result<Self> {
        let issuer = issuer.trim_end_matches('/');
        if !issuer.starts_with("https://") {
            return Err(Error::Di(format!(
                "OIDC issuer must be https:// (got '{issuer}')"
            )));
        }
        let url = format!("{issuer}/.well-known/openid-configuration");
        let value = http.get_json(&url).await?;
        let document: Self = serde_json::from_value(value).map_err(|error| {
            Error::Status(503, format!("invalid OIDC discovery document: {error}"))
        })?;
        document.validate_with(issuer, require_pkce_advertised)?;
        Ok(document)
    }

    /// Fail-closed validation of the document.
    pub fn validate(&self, requested_issuer: &str) -> Result<()> {
        self.validate_with(requested_issuer, true)
    }

    /// Validation with an explicit PKCE-advertisement policy.
    pub fn validate_with(
        &self,
        requested_issuer: &str,
        require_pkce_advertised: bool,
    ) -> Result<()> {
        let requested = requested_issuer.trim_end_matches('/');
        if self.issuer.trim_end_matches('/') != requested {
            return Err(Error::Di(format!(
                "OIDC discovery issuer mismatch: requested '{requested}', document '{}'",
                self.issuer
            )));
        }
        for (label, endpoint) in [
            ("authorization_endpoint", &self.authorization_endpoint),
            ("token_endpoint", &self.token_endpoint),
            ("jwks_uri", &self.jwks_uri),
        ] {
            if !endpoint.starts_with("https://") {
                return Err(Error::Di(format!(
                    "OIDC {label} must be https:// (got '{endpoint}')"
                )));
            }
        }
        let advertises_s256 = self
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256");
        if !advertises_s256
            && (require_pkce_advertised || !self.code_challenge_methods_supported.is_empty())
        {
            return Err(Error::Di(
                "OIDC provider does not advertise PKCE S256".to_string(),
            ));
        }
        if !self
            .response_types_supported
            .iter()
            .any(|response_type| response_type == "code")
        {
            return Err(Error::Di(
                "OIDC provider does not advertise the authorization-code flow".to_string(),
            ));
        }
        if self.id_token_signing_alg_values_supported.is_empty() {
            return Err(Error::Di(
                "OIDC provider does not advertise id_token signing algorithms".to_string(),
            ));
        }
        Ok(())
    }
}
