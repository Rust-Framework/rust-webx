//! OIDC relying-party client: authorization URL, code exchange, id_token
//! validation and the session bridge.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jsonwebtoken::{decode, decode_header, Algorithm, Validation};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use url::Url;
use webx_core::config::JwtAlgorithm;
use webx_core::error::{Error, Result};
use webx_core::security::{constant_time_eq, TokenClaims};
use webx_host::auth_jwks::{IJwksFetcher, IJwtKeyResolver, JwkSet, JwksResolver};

use crate::discovery::OidcDiscovery;
use crate::http::IOidcHttp;
use crate::pkce::Pkce;

/// Client authentication method at the token endpoint.
#[derive(Clone)]
pub enum ClientAuth {
    /// Public client (PKCE only).
    None,
    /// `client_secret_post`.
    ClientSecretPost(String),
    /// `client_secret_basic`.
    ClientSecretBasic(String),
}

impl fmt::Debug for ClientAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("ClientAuth::None"),
            Self::ClientSecretPost(_) => {
                formatter.write_str("ClientAuth::ClientSecretPost(<redacted>)")
            }
            Self::ClientSecretBasic(_) => {
                formatter.write_str("ClientAuth::ClientSecretBasic(<redacted>)")
            }
        }
    }
}

/// Relying-party behaviour knobs.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Accepted id_token algorithms (HS* is rejected).
    pub algorithms: Vec<JwtAlgorithm>,
    /// Clock-skew allowance.
    pub leeway: Duration,
    /// JWKS cache TTL.
    pub jwks_ttl: Duration,
    /// Maximum keys kept from the JWKS document.
    pub max_keys: usize,
    /// Default scopes; `openid` is always included.
    pub scopes: Vec<String>,
    /// Require the provider to advertise PKCE S256 (default `true`).
    ///
    /// Set `false` only for providers known to support PKCE without
    /// advertising it (e.g. Microsoft multi-tenant discovery).
    pub require_pkce_advertised: bool,
}

impl OidcConfig {
    /// Fail-closed bounds for RP configuration.
    pub fn validate(&self) -> Result<()> {
        if self.algorithms.is_empty() {
            return Err(Error::Di("OIDC algorithms must not be empty".to_string()));
        }
        if self.leeway > Duration::from_secs(300) {
            return Err(Error::Di(
                "OIDC leeway must be at most 300 seconds".to_string(),
            ));
        }
        if self.jwks_ttl.is_zero() || self.jwks_ttl > Duration::from_secs(3600) {
            return Err(Error::Di(
                "OIDC jwks_ttl must be between 1 and 3600 seconds".to_string(),
            ));
        }
        if self.max_keys == 0 || self.max_keys > 128 {
            return Err(Error::Di(
                "OIDC max_keys must be between 1 and 128".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            algorithms: vec![JwtAlgorithm::Rs256, JwtAlgorithm::Es256],
            leeway: Duration::from_secs(30),
            jwks_ttl: Duration::from_secs(300),
            max_keys: 32,
            scopes: vec!["openid".into(), "profile".into(), "email".into()],
            require_pkce_advertised: true,
        }
    }
}

/// An OIDC relying-party client bound to one issuer/client id.
pub struct OidcClient {
    discovery: OidcDiscovery,
    client_id: String,
    client_auth: ClientAuth,
    http: Arc<dyn IOidcHttp>,
    resolver: Arc<dyn IJwtKeyResolver>,
    config: OidcConfig,
}

impl OidcClient {
    /// Discover and validate the provider, then build the client with defaults.
    pub async fn discover(
        http: Arc<dyn IOidcHttp>,
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        client_auth: ClientAuth,
    ) -> Result<Self> {
        Self::discover_with(http, issuer, client_id, client_auth, OidcConfig::default()).await
    }

    /// Like [`discover`](Self::discover) with an explicit configuration.
    pub async fn discover_with(
        http: Arc<dyn IOidcHttp>,
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        client_auth: ClientAuth,
        mut config: OidcConfig,
    ) -> Result<Self> {
        let issuer = issuer.into();
        let client_id = client_id.into();
        config.validate()?;
        if client_id.trim().is_empty() {
            return Err(Error::Di("OIDC client_id must not be empty".to_string()));
        }
        let discovery =
            OidcDiscovery::fetch_with(http.as_ref(), &issuer, config.require_pkce_advertised)
                .await?;

        // Only asymmetric algorithms make sense for an id_token we verify
        // against a remote JWKS; HS* would need the client secret as MAC key.
        config.algorithms.retain(|algorithm| {
            *algorithm != JwtAlgorithm::Hs256
                && discovery
                    .id_token_signing_alg_values_supported
                    .iter()
                    .any(|supported| supported == algorithm.as_str())
        });
        if config.algorithms.is_empty() {
            return Err(Error::Di(
                "OIDC provider and OidcConfig share no asymmetric id_token algorithm".to_string(),
            ));
        }

        let fetcher = Arc::new(OidcJwksFetcher {
            http: Arc::clone(&http),
            url: discovery.jwks_uri.clone(),
        });
        let resolver = Arc::new(JwksResolver::new(fetcher, config.jwks_ttl, config.max_keys));
        Ok(Self {
            discovery,
            client_id,
            client_auth,
            http,
            resolver,
            config,
        })
    }

    /// Discovery document in effect.
    pub fn discovery(&self) -> &OidcDiscovery {
        &self.discovery
    }

    /// Build an authorization URL and the server-side values to remember.
    pub fn authorization_url(&self, request: AuthorizationRequest) -> Result<AuthorizationUrl> {
        if matches!(self.client_auth, ClientAuth::None) && !request.pkce {
            return Err(Error::Validation(
                "public OIDC clients must enable PKCE S256".to_string(),
            ));
        }
        let state = request.state.unwrap_or_else(new_id);
        let nonce = request.nonce.unwrap_or_else(new_id);
        let pkce = request.pkce.then(Pkce::generate);
        let mut scopes = request.scopes.unwrap_or_else(|| self.config.scopes.clone());
        if !scopes.iter().any(|scope| scope == "openid") {
            scopes.insert(0, "openid".to_string());
        }

        let mut url = Url::parse(&self.discovery.authorization_endpoint)
            .map_err(|error| Error::Di(format!("invalid OIDC authorization endpoint: {error}")))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &self.client_id);
            query.append_pair("redirect_uri", &request.redirect_uri);
            query.append_pair("scope", &scopes.join(" "));
            query.append_pair("state", &state);
            query.append_pair("nonce", &nonce);
            if let Some(pkce) = &pkce {
                query.append_pair("code_challenge", pkce.challenge());
                query.append_pair("code_challenge_method", "S256");
            }
            for (key, value) in &request.extra {
                if !RESERVED_PARAMS.contains(&key.as_str()) {
                    query.append_pair(key, value);
                }
            }
        }

        Ok(AuthorizationUrl {
            url: url.to_string(),
            state,
            nonce,
            pkce_verifier: pkce.map(|pkce| pkce.verifier().to_string()),
        })
    }

    /// Exchange an authorization code and validate the returned id_token.
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        pkce_verifier: Option<&str>,
        expected_nonce: Option<&str>,
    ) -> Result<OidcSession> {
        if code.trim().is_empty() {
            return Err(Error::Validation(
                "authorization code must not be empty".to_string(),
            ));
        }
        // Public clients are PKCE-only: without a verifier the token request
        // (and therefore the code) is replayable by anyone who intercepts it.
        if matches!(self.client_auth, ClientAuth::None) && pkce_verifier.is_none() {
            return Err(Error::Validation(
                "public OIDC clients must send a PKCE code_verifier".to_string(),
            ));
        }

        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", self.client_id.clone()),
        ];
        if let Some(verifier) = pkce_verifier {
            form.push(("code_verifier", verifier.to_string()));
        }
        let mut headers: Vec<(&str, String)> = Vec::new();
        match &self.client_auth {
            ClientAuth::None => {}
            ClientAuth::ClientSecretPost(secret) => {
                form.push(("client_secret", secret.clone()));
            }
            ClientAuth::ClientSecretBasic(secret) => {
                let credentials = format!("{}:{secret}", self.client_id);
                let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
                headers.push(("authorization", format!("Basic {encoded}")));
            }
        }

        let value = self
            .http
            .post_form(&self.discovery.token_endpoint, &form, &headers)
            .await?;
        let token: TokenResponse = serde_json::from_value(value)
            .map_err(|error| Error::Status(502, format!("invalid OIDC token response: {error}")))?;
        let id_token = token.id_token.ok_or_else(|| {
            Error::Status(502, "OIDC token response is missing id_token".to_string())
        })?;

        let claims = self
            .validate_id_token(&id_token, expected_nonce, token.access_token.as_deref())
            .await?;
        let now = unix_now();
        let expires_at_unix_secs = token
            .expires_in
            .map(|seconds| now.saturating_add(seconds))
            .unwrap_or(claims.exp);

        let name = claims
            .name
            .clone()
            .or_else(|| claims.preferred_username.clone());
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), claims.sub.clone());
        raw.insert("iss".to_string(), claims.iss.clone());
        if let Some(email) = &claims.email {
            raw.insert("email".to_string(), email.clone());
        }
        if let Some(name) = &name {
            raw.insert("name".to_string(), name.clone());
        }
        if let Some(acr) = &claims.acr {
            raw.insert("acr".to_string(), acr.clone());
        }
        if !claims.amr.is_empty() {
            raw.insert("amr".to_string(), claims.amr.join(","));
        }

        Ok(OidcSession {
            subject: claims.sub,
            email: claims.email,
            name,
            id_token,
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at_unix_secs,
            claims: raw,
        })
    }

    async fn validate_id_token(
        &self,
        id_token: &str,
        expected_nonce: Option<&str>,
        access_token: Option<&str>,
    ) -> Result<IdTokenClaims> {
        let header = decode_header(id_token)
            .map_err(|error| Error::Unauthorized(format!("invalid id_token header: {error}")))?;
        if !self
            .config
            .algorithms
            .iter()
            .any(|algorithm| jsonwebtoken_algorithm(*algorithm) == header.alg)
        {
            return Err(Error::Unauthorized(format!(
                "id_token algorithm {:?} is not allowed",
                header.alg
            )));
        }

        let kid = header.kid.as_deref().unwrap_or("");
        let key = match self.resolver.resolve(kid, header.alg).await {
            Ok(key) => key,
            Err(error) if error.status_code() == 503 => return Err(error),
            Err(_) => {
                return Err(Error::Unauthorized(
                    "id_token key id is unknown".to_string(),
                ))
            }
        };

        let mut validation = Validation::new(header.alg);
        validation.leeway = self.config.leeway.as_secs();
        validation.validate_nbf = true;
        validation.set_issuer(&[self.discovery.issuer.as_str()]);
        validation.set_audience(&[self.client_id.as_str()]);
        validation.required_spec_claims = ["exp", "iss", "aud", "sub"]
            .into_iter()
            .map(String::from)
            .collect::<HashSet<_>>();

        let data = decode::<IdTokenClaims>(id_token, key.as_ref(), &validation)
            .map_err(|error| Error::Unauthorized(format!("id_token rejected: {error}")))?;
        let claims = data.claims;

        if claims.sub.trim().is_empty() {
            return Err(Error::Unauthorized(
                "id_token has an empty subject".to_string(),
            ));
        }
        if let Some(expected) = expected_nonce {
            if claims.nonce.as_deref() != Some(expected) {
                return Err(Error::Unauthorized("id_token nonce mismatch".to_string()));
            }
        }
        let multiple_audiences =
            matches!(&claims.aud, Value::Array(audiences) if audiences.len() > 1);
        if multiple_audiences && claims.azp.as_deref() != Some(self.client_id.as_str()) {
            return Err(Error::Unauthorized(
                "id_token azp does not match the client".to_string(),
            ));
        }
        if let Some(issued_at) = claims.iat {
            if issued_at > unix_now().saturating_add(self.config.leeway.as_secs()) {
                return Err(Error::Unauthorized(
                    "id_token was issued in the future".to_string(),
                ));
            }
        }
        if let Some(at_hash) = &claims.at_hash {
            let Some(access_token) = access_token else {
                return Err(Error::Unauthorized(
                    "id_token at_hash is present but no access_token was returned".to_string(),
                ));
            };
            let expected = access_token_hash(header.alg, access_token)?;
            if !constant_time_eq(at_hash.as_bytes(), expected.as_bytes()) {
                return Err(Error::Unauthorized("id_token at_hash mismatch".to_string()));
            }
        }
        Ok(claims)
    }
}

/// Authorization request inputs.
#[derive(Debug, Clone)]
pub struct AuthorizationRequest {
    redirect_uri: String,
    scopes: Option<Vec<String>>,
    state: Option<String>,
    nonce: Option<String>,
    pkce: bool,
    extra: Vec<(String, String)>,
}

impl AuthorizationRequest {
    /// A request with PKCE disabled until [`pkce_s256`](Self::pkce_s256).
    pub fn new(redirect_uri: impl Into<String>) -> Self {
        Self {
            redirect_uri: redirect_uri.into(),
            scopes: None,
            state: None,
            nonce: None,
            pkce: false,
            extra: Vec::new(),
        }
    }

    /// Enable PKCE S256.
    pub fn pkce_s256(mut self) -> Self {
        self.pkce = true;
        self
    }

    /// Use a caller-generated, stored state value.
    pub fn state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }

    /// Use a caller-generated nonce.
    pub fn nonce(mut self, nonce: impl Into<String>) -> Self {
        self.nonce = Some(nonce.into());
        self
    }

    /// Override the default scopes.
    pub fn scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = Some(scopes.into_iter().map(Into::into).collect());
        self
    }

    /// Append a non-reserved parameter (e.g. `prompt=login`).
    pub fn extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.push((key.into(), value.into()));
        self
    }
}

/// Where to redirect the browser and what to remember server-side.
#[derive(Clone)]
pub struct AuthorizationUrl {
    /// Authorization endpoint URL.
    pub url: String,
    /// State to verify on the callback.
    pub state: String,
    /// Nonce to verify in the id_token.
    pub nonce: String,
    /// PKCE verifier to send at the token endpoint (absent when PKCE off).
    pub pkce_verifier: Option<String>,
}

impl fmt::Debug for AuthorizationUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationUrl")
            .field("url", &self.url)
            .field("state", &self.state)
            .field("nonce", &self.nonce)
            .field(
                "pkce_verifier",
                &self.pkce_verifier.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// A validated OIDC login.
#[derive(Clone, PartialEq, Eq)]
pub struct OidcSession {
    /// `sub` from the id_token.
    pub subject: String,
    /// Email claim, when present.
    pub email: Option<String>,
    /// Display name (or preferred_username).
    pub name: Option<String>,
    /// Raw id_token (for audit; do not log it).
    pub id_token: String,
    /// Access token, when the provider returned one.
    pub access_token: Option<String>,
    /// Refresh token, when the provider issued one.
    pub refresh_token: Option<String>,
    /// Session expiry (Unix seconds).
    pub expires_at_unix_secs: u64,
    /// Normalized claims (sub/iss/email/name/acr/amr).
    pub claims: HashMap<String, String>,
}

impl fmt::Debug for OidcSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OidcSession")
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("name", &self.name)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .field("id_token", &"<redacted>")
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl OidcSession {
    /// Convert into framework [`TokenClaims`] for `ITokenIssuer`.
    ///
    /// The host still decides roles/permissions/tenant; the OIDC client only
    /// carries identity attributes.
    pub fn to_token_claims(&self) -> TokenClaims {
        let mut claims = TokenClaims::new(&self.subject);
        if let Some(name) = &self.name {
            claims = claims.with_claim("name", name.clone());
        }
        if let Some(email) = &self.email {
            claims = claims.with_claim("email", email.clone());
        }
        if let Some(acr) = self.claims.get("acr") {
            claims = claims.with_claim("acr", acr.clone());
        }
        if let Some(amr) = self.claims.get("amr") {
            claims = claims.with_claim("amr", amr.clone());
        }
        claims
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    iss: String,
    aud: Value,
    exp: u64,
    #[serde(default)]
    iat: Option<u64>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    acr: Option<String>,
    #[serde(default)]
    amr: Vec<String>,
    #[serde(default)]
    at_hash: Option<String>,
}

struct OidcJwksFetcher {
    http: Arc<dyn IOidcHttp>,
    url: String,
}

#[async_trait::async_trait]
impl IJwksFetcher for OidcJwksFetcher {
    async fn fetch(&self) -> Result<JwkSet> {
        let value = self.http.get_json(&self.url).await?;
        serde_json::from_value(value)
            .map_err(|error| Error::Status(503, format!("invalid OIDC JWKS: {error}")))
    }
}

const RESERVED_PARAMS: &[&str] = &[
    "response_type",
    "client_id",
    "redirect_uri",
    "scope",
    "state",
    "nonce",
    "code_challenge",
    "code_challenge_method",
];

fn access_token_hash(algorithm: Algorithm, access_token: &str) -> Result<String> {
    // OIDC at_hash: hash with the id_token's hash algorithm, then the left half.
    let digest = match algorithm {
        Algorithm::RS256 | Algorithm::PS256 | Algorithm::ES256 => {
            Sha256::digest(access_token.as_bytes()).to_vec()
        }
        _ => {
            return Err(Error::Unauthorized(
                "unsupported id_token algorithm for at_hash".to_string(),
            ))
        }
    };
    let half = &digest[..digest.len() / 2];
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(half))
}

fn jsonwebtoken_algorithm(algorithm: JwtAlgorithm) -> Algorithm {
    match algorithm {
        JwtAlgorithm::Hs256 => Algorithm::HS256,
        JwtAlgorithm::Rs256 => Algorithm::RS256,
        JwtAlgorithm::Es256 => Algorithm::ES256,
        JwtAlgorithm::Ps256 => Algorithm::PS256,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
