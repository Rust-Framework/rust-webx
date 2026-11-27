//! OAuth 2.0 `client_credentials` grant: service-to-service machine tokens.
//!
//! The authorization-code flow in [`crate::client`] represents an end user.
//! This module represents a **service account**: the application authenticates
//! as itself (RFC 6749 §4.4) and receives a machine token it presents to
//! another service. Nothing here carries a user identity.
//!
//! ```rust,ignore
//! let config = ClientCredentialsConfig::new(
//!     "https://id.example.com/oauth2/token",
//!     "orders-service",
//!     secret_from_secret_store,
//!     ["orders.write"],
//! )
//! .with_audience("https://billing.example.com");
//! let client = ClientCredentialsClient::new(config, http, clock)?;
//!
//! let token = client.token().await?;
//! // `token.access_token` is cached and refreshed `refresh_ahead` before expiry
//! send(OutboundRequest::get(url).header("authorization", format!("Bearer {}", token.access_token)))
//!     .await?;
//! ```
//!
//! Failure semantics are fail-closed, and both halves are never cached:
//!
//! * a `4xx` verdict (`invalid_client`, `invalid_scope`, ...) is a credential or
//!   configuration defect — [`Error::di`] (`500`, not retryable);
//! * `5xx`, `429` throttling, transport failures, deadlines and malformed
//!   responses are transient — [`Error::unavailable`] (`503`, retryable).
//!
//! There is no degraded path: the caller either gets a token or an error, never
//! an empty token and never a request sent without authentication.
//!
//! [`ClientCredentialsClient`] also implements the framework port
//! [`IClientCredentials`], so business code can depend on
//! `Arc<dyn IClientCredentials>` (one DI singleton) instead of this adapter.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::timeout;
use url::Url;
use webx_client::{HttpClientError, IHttpClient, OutboundRequest};
use webx_core::error::{Error, Result};
use webx_core::security::IClientCredentials;
use webx_core::time::IClock;

/// The machine token, defined once in `rust-webx-core`
/// ([`webx_core::security::ServiceToken`]) so call sites depend on the port, not
/// on this adapter.
pub use webx_core::security::ServiceToken;

/// Token endpoint responses are small JSON documents; anything larger is
/// treated as a malformed response.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Conservatively assumed lifetime when the endpoint omits `expires_in`
/// (RFC 6749 makes it optional; a far longer guess would outlive the token).
const DEFAULT_EXPIRES_IN_SECS: u64 = 300;

/// Refresh this long before expiry unless the token is shorter-lived.
const DEFAULT_REFRESH_AHEAD: Duration = Duration::from_secs(60);

/// Default dead line for one token request.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A token request must not be abandoned sooner than this: a cold TLS
/// handshake plus a token endpoint round trip needs a real budget.
const MIN_TIMEOUT: Duration = Duration::from_secs(1);

/// A token request must not pin a caller longer than this; the grant is a
/// request-path operation, not a batch job.
const MAX_TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound on cached `(scope set, audience)` buckets.
///
/// Callers choose the scope set, so an unbounded map would let caller-chosen
/// keys grow the process without limit. 256 is far above any fixed set of
/// service-to-service audiences and small enough to stay negligible.
const DEFAULT_MAX_CACHE_ENTRIES: usize = 256;

/// How the token endpoint authenticates the client (RFC 6749 §2.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientSecretAuth {
    /// `Authorization: Basic base64(form(client_id) ":" form(client_secret))`.
    ///
    /// Credentials stay out of the body and out of access logs that record it.
    #[default]
    Basic,
    /// `client_id` / `client_secret` in the form body
    /// (`client_secret_post`), for endpoints that reject HTTP Basic.
    Post,
}

/// Client credentials plus token-endpoint settings.
#[derive(Clone)]
pub struct ClientCredentialsConfig {
    /// Absolute token endpoint URL. Must be `https://` unless
    /// [`allow_private_networks`](Self::allow_private_networks) is enabled.
    pub token_endpoint: String,
    /// The registered client id (`client_id`).
    pub client_id: String,
    /// The client secret. Never rendered by `Debug`.
    pub client_secret: String,
    /// How the client authenticates at the token endpoint.
    pub client_auth: ClientSecretAuth,
    /// Optional `audience` form parameter (resource indicator, RFC 8707).
    pub audience: Option<String>,
    /// Scopes sent as the space-separated `scope` parameter.
    pub scopes: Vec<String>,
    /// Refresh this far before expiry, clamped to `[0, expires_in / 2]`.
    pub refresh_ahead: Duration,
    /// Deadline for one token request, clamped to `[1s, 60s]`.
    pub timeout: Duration,
    /// Allow a plaintext loopback token endpoint (tests, local IdP).
    pub allow_private_networks: bool,
}

impl fmt::Debug for ClientCredentialsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientCredentialsConfig")
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("client_auth", &self.client_auth)
            .field("audience", &self.audience)
            .field("scopes", &self.scopes)
            .field("refresh_ahead", &self.refresh_ahead)
            .field("timeout", &self.timeout)
            .field("allow_private_networks", &self.allow_private_networks)
            .finish()
    }
}

impl ClientCredentialsConfig {
    /// Configuration for one token endpoint and one service account.
    pub fn new(
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            client_auth: ClientSecretAuth::Basic,
            audience: None,
            scopes: scopes.into_iter().map(Into::into).collect(),
            refresh_ahead: DEFAULT_REFRESH_AHEAD,
            timeout: DEFAULT_TIMEOUT,
            allow_private_networks: false,
        }
    }

    /// Send an `audience` form parameter (RFC 8707 resource indicator).
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// Select `client_secret_basic` (default) or `client_secret_post`.
    pub fn with_client_auth(mut self, auth: ClientSecretAuth) -> Self {
        self.client_auth = auth;
        self
    }

    /// Allow a plaintext loopback token endpoint (tests, local IdP).
    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }

    /// Refresh `refresh_ahead` before expiry (clamped to `[0, expires_in / 2]`
    /// once the token lifetime is known).
    pub fn with_refresh_ahead(mut self, refresh_ahead: Duration) -> Self {
        self.refresh_ahead = refresh_ahead;
        self
    }

    /// Set the per-request deadline (clamped to `[1s, 60s]`).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// A machine-token client for one service account.
///
/// Tokens are cached per `(scope set, audience)`: a cached token is returned
/// while `now < expires_at - refresh_ahead`, and concurrent callers for the same
/// key share a single in-flight request (single-flight).
///
/// Locking is two-level on purpose. The outer map mutex is held only for a hash
/// lookup and is released before any `await`; the per-key mutex is held across
/// the token request, which is what serializes concurrent callers of the *same*
/// key. A single global mutex held across the request would instead serialize
/// unrelated scopes and audiences — and every token request would head-of-line
/// block all other outbound work in the process. A caller queued behind a
/// *failing* leader retries on its own, because failures are never cached.
///
/// The value is `Send + Sync` and cheap to wrap in an `Arc`.
///
/// The cache holds at most 256 buckets. When a new key would exceed that, the
/// oldest *inserted* bucket is dropped (insertion order, not least-recently
/// used). Evicting a bucket never cancels a request already in flight on it;
/// that request's result is written to a bucket no longer reachable from the
/// map, so the next caller for the key simply requests again.
pub struct ClientCredentialsClient {
    config: ClientCredentialsConfig,
    http: Arc<dyn IHttpClient>,
    clock: Arc<dyn IClock>,
    cache: Mutex<Cache>,
}

impl fmt::Debug for ClientCredentialsClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientCredentialsClient")
            .field("config", &self.config)
            .finish()
    }
}

impl ClientCredentialsClient {
    /// Build a client over the framework HTTP client.
    ///
    /// [`IHttpClient`] is the only transport seam: production injects
    /// [`HttpClient`](webx_client::HttpClient), tests inject a fake that returns
    /// [`OutboundResponse::from_bytes`](webx_client::OutboundResponse::from_bytes)
    /// / [`ok_response`](webx_client::testing::ok_response).
    ///
    /// Endpoint shape and the required credentials are validated here, so a
    /// misconfigured service fails at start-up instead of on its first call.
    /// Every failure is an [`Error::di`] (configuration defect, `500`).
    ///
    /// The client's base URL is irrelevant (the token endpoint is absolute),
    /// but its own SSRF policy still applies: `config.allow_private_networks`
    /// gates this client's `https://` check only, so an intranet endpoint also
    /// needs an `IHttpClient` built with
    /// `HttpClientOptions::allow_private_networks`.
    pub fn new(
        config: ClientCredentialsConfig,
        http: Arc<dyn IHttpClient>,
        clock: Arc<dyn IClock>,
    ) -> Result<Self> {
        validate_config(&config)?;
        Ok(Self {
            config,
            http,
            clock,
            cache: Mutex::new(Cache::new()),
        })
    }

    /// A token for the configured scopes and audience.
    ///
    /// Returns the cached token when it is still inside its validity window;
    /// otherwise requests one. Failures are never cached.
    pub async fn token(&self) -> Result<ServiceToken> {
        let scopes = normalize_scopes(self.config.scopes.iter().map(String::as_str));
        self.token_scoped(scopes).await
    }

    /// A token for `scopes`, overriding the configured scopes.
    ///
    /// An empty slice requests a token **without** a `scope` parameter (the
    /// endpoint's default scope set); it is a different cache bucket from the
    /// configured scopes, not an alias for them.
    pub async fn token_for(&self, scopes: &[&str]) -> Result<ServiceToken> {
        let scopes = normalize_scopes(scopes.iter().copied());
        self.token_scoped(scopes).await
    }

    /// Drop every cached token.
    ///
    /// Call this after a credential rotation, or when a resource server
    /// answers `401` for a token this client minted. The next call requests a
    /// new token; a request already in flight is not cancelled and its result
    /// is not served afterwards.
    pub async fn invalidate(&self) {
        self.cache.lock().await.clear();
    }

    /// Number of cached token buckets.
    ///
    /// Diagnostic only: this is a snapshot under the cache lock, so it is not a
    /// precise instantaneous value with respect to concurrent callers.
    pub async fn cached_entries(&self) -> usize {
        self.cache.lock().await.len()
    }

    async fn token_scoped(&self, scopes: Vec<String>) -> Result<ServiceToken> {
        let key = CacheKey::new(&scopes, self.config.audience.as_ref());
        let bucket = self.bucket(&key).await;

        // Held across the request: this is the single-flight gate for `key`.
        let mut slot = bucket.lock().await;
        if let Some(cached) = slot.as_ref() {
            if self.clock.now_unix() < cached.refresh_at_unix_secs {
                return Ok(cached.token.clone());
            }
        }
        let token = self.request_token(&scopes).await?;
        let refresh_at_unix_secs = self.refresh_at(&token);
        *slot = Some(CachedToken {
            token: token.clone(),
            refresh_at_unix_secs,
        });
        Ok(token)
    }

    /// The per-key bucket, creating it on first use.
    ///
    /// The map guard is confined to this function: it is dropped before the
    /// caller locks the bucket, so the global lock is never held across an
    /// `await` and never orders two token requests. Creating a bucket may evict
    /// the oldest inserted key while this lock is held — no bucket lock is ever
    /// taken under it.
    async fn bucket(&self, key: &CacheKey) -> Bucket {
        let mut cache = self.cache.lock().await;
        match cache.get(key) {
            Some(bucket) => Arc::clone(bucket),
            None => {
                let bucket: Bucket = Arc::new(Mutex::new(None));
                cache.insert(key.clone(), Arc::clone(&bucket));
                bucket
            }
        }
    }

    fn refresh_at(&self, token: &ServiceToken) -> u64 {
        let lifetime = token
            .expires_at_unix_secs
            .saturating_sub(self.clock.now_unix());
        let ahead = self.config.refresh_ahead.as_secs().min(lifetime / 2);
        token.expires_at_unix_secs.saturating_sub(ahead)
    }

    fn effective_timeout(&self) -> Duration {
        self.config.timeout.clamp(MIN_TIMEOUT, MAX_TIMEOUT)
    }

    /// One token exchange, bounded end to end.
    ///
    /// `OutboundRequest::timeout` is only a hint the injected
    /// [`IHttpClient`] may ignore (or honour per chunk), so the transport hint
    /// is not a deadline: execute, body read and parse all run under one
    /// `tokio::time::timeout`, which also releases the per-key lock the caller
    /// holds across this call. A cancelled request leaves nothing cached.
    async fn request_token(&self, scopes: &[String]) -> Result<ServiceToken> {
        let deadline = self.effective_timeout();
        let (form, headers) = self.build_request(scopes);
        let mut request = OutboundRequest::post(self.config.token_endpoint.trim())
            .timeout(deadline)
            .body(form);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let exchange = async {
            let response = self.http.execute(request).await.map_err(transport_error)?;
            let status = response.status;
            let body = response
                .body
                .collect(MAX_BODY_BYTES)
                .await
                .map_err(transport_error)?;
            if !(200..300).contains(&status) {
                return Err(status_error(status, &body));
            }
            self.parse_token(&body, scopes)
        };
        match timeout(deadline, exchange).await {
            Ok(result) => result,
            Err(_elapsed) => Err(Error::unavailable("the OAuth2 token request timed out")),
        }
    }

    fn build_request(&self, scopes: &[String]) -> (String, Vec<(String, String)>) {
        let mut params: Vec<(String, String)> =
            vec![("grant_type".to_string(), "client_credentials".to_string())];
        if !scopes.is_empty() {
            params.push(("scope".to_string(), scopes.join(" ")));
        }
        if let Some(audience) = &self.config.audience {
            params.push(("audience".to_string(), audience.clone()));
        }
        let mut headers = vec![
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ];
        match self.config.client_auth {
            ClientSecretAuth::Basic => {
                let credentials =
                    basic_credentials(&self.config.client_id, &self.config.client_secret);
                headers.push(("authorization".to_string(), format!("Basic {credentials}")));
            }
            ClientSecretAuth::Post => {
                params.push(("client_id".to_string(), self.config.client_id.clone()));
                params.push((
                    "client_secret".to_string(),
                    self.config.client_secret.clone(),
                ));
            }
        }
        (form_encode(&params), headers)
    }

    fn parse_token(&self, body: &[u8], scopes: &[String]) -> Result<ServiceToken> {
        let value: Value = serde_json::from_slice(body).map_err(|_| {
            Error::unavailable("the OAuth2 token endpoint returned an invalid JSON body")
        })?;
        let access_token = match value.get("access_token") {
            Some(Value::String(token)) if !token.trim().is_empty() => token.trim().to_string(),
            _ => {
                return Err(Error::unavailable(
                    "the OAuth2 token endpoint returned no access token",
                ))
            }
        };
        // RFC 6749 §7.1: `token_type` is case-insensitive; anything other than
        // Bearer is a token this framework cannot use, so it is rejected.
        match value.get("token_type") {
            None => {}
            Some(Value::String(token_type)) if token_type.trim().eq_ignore_ascii_case("Bearer") => {
            }
            Some(_) => {
                return Err(Error::unavailable(
                    "the OAuth2 token endpoint returned a non-Bearer token",
                ))
            }
        }
        let expires_in = expires_in(&value).unwrap_or(DEFAULT_EXPIRES_IN_SECS);
        let scope = match value.get("scope") {
            Some(Value::String(scope)) if !scope.trim().is_empty() => scope.trim().to_string(),
            _ => scopes.join(" "),
        };
        Ok(ServiceToken {
            access_token,
            expires_at_unix_secs: self.clock.now_unix().saturating_add(expires_in),
            scope,
        })
    }
}

/// The framework port: one DI singleton can be shared by every outbound
/// adapter, and all of them observe the same keyed token cache.
#[async_trait]
impl IClientCredentials for ClientCredentialsClient {
    async fn token(&self, scope: &[&str]) -> Result<ServiceToken> {
        self.token_for(scope).await
    }
}

/// One cached token plus the instant it stops being served.
struct CachedToken {
    token: ServiceToken,
    refresh_at_unix_secs: u64,
}

type Bucket = Arc<Mutex<Option<CachedToken>>>;

/// Bounded bucket map: `entries` for lookup, `order` for insertion-order
/// eviction. Both are only touched under the client's cache mutex.
struct Cache {
    entries: HashMap<CacheKey, Bucket>,
    order: VecDeque<CacheKey>,
    max_entries: usize,
}

impl Cache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            max_entries: DEFAULT_MAX_CACHE_ENTRIES,
        }
    }

    fn get(&self, key: &CacheKey) -> Option<&Bucket> {
        self.entries.get(key)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Insert a bucket for a key that is not cached yet, dropping the oldest
    /// inserted key when the map is full.
    fn insert(&mut self, key: CacheKey, bucket: Bucket) {
        while self.entries.len() >= self.max_entries {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, bucket);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// Cache identity: the scope set (order-independent) and the audience.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    scope: String,
    audience: Option<String>,
}

impl CacheKey {
    fn new(scopes: &[String], audience: Option<&String>) -> Self {
        Self {
            scope: scopes.join(" "),
            audience: audience.cloned(),
        }
    }
}

/// Trim, drop empties, then sort and dedupe so the same scope set is one key
/// (and one deterministic `scope` parameter) regardless of call-site order.
fn normalize_scopes<'a>(scopes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut scopes: Vec<String> = scopes
        .into_iter()
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(str::to_string)
        .collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

fn validate_config(config: &ClientCredentialsConfig) -> Result<()> {
    if config.client_id.trim().is_empty() {
        return Err(Error::di(
            "OAuth2 client-credentials client_id must not be empty",
        ));
    }
    if config.client_secret.trim().is_empty() {
        return Err(Error::di(
            "OAuth2 client-credentials client_secret must not be empty",
        ));
    }
    let endpoint = config.token_endpoint.trim();
    if endpoint.is_empty() {
        return Err(Error::di(
            "OAuth2 client-credentials token_endpoint must not be empty",
        ));
    }
    let url = Url::parse(endpoint).map_err(|error| {
        Error::di(format!(
            "OAuth2 client-credentials token_endpoint is not an absolute URL: {error}"
        ))
    })?;
    let loopback = url.host_str().is_some_and(is_loopback_host);
    match url.scheme() {
        "https" => Ok(()),
        "http" if config.allow_private_networks && loopback => Ok(()),
        "http" => Err(Error::di(
            "OAuth2 client-credentials token_endpoint must be https:// unless private networks are allowed (plaintext is accepted for loopback only)",
        )),
        scheme => Err(Error::di(format!(
            "OAuth2 client-credentials token_endpoint must use https:// (got scheme '{scheme}')"
        ))),
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.is_loopback())
}

/// `client_secret_basic` credentials: form-urlencode both halves, then
/// base64 the `id:secret` pair (RFC 6749 §2.3.1).
fn basic_credentials(client_id: &str, client_secret: &str) -> String {
    let raw = format!(
        "{}:{}",
        form_encode_component(client_id),
        form_encode_component(client_secret)
    );
    base64::engine::general_purpose::STANDARD.encode(raw)
}

fn form_encode_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn form_encode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                form_encode_component(key),
                form_encode_component(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn expires_in(value: &Value) -> Option<u64> {
    match value.get("expires_in")? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// The provider's error identifier (`{"error":"invalid_client"}`), kept only
/// when it is a short fixed-shape token: it lands in an operator-only message,
/// never in a client-visible detail.
fn provider_error_code(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let code = value.get("error")?.as_str()?;
    let safe = !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    safe.then(|| code.to_string())
}

/// `4xx` is a credential/configuration verdict: no retry, no cache.
///
/// `429 Too Many Requests` is the exception: it is the endpoint throttling us,
/// not a verdict on the credentials, so it stays transient (`503`, retryable)
/// and is never cached either.
fn status_error(status: u16, body: &[u8]) -> Error {
    if status == 429 {
        return Error::unavailable(
            "the OAuth2 token endpoint is rate limiting token requests (HTTP 429)",
        );
    }
    if (400..500).contains(&status) {
        return Error::di(match provider_error_code(body) {
            Some(code) => format!(
                "the OAuth2 token endpoint rejected the client credentials (HTTP {status}, error {code})"
            ),
            None => format!(
                "the OAuth2 token endpoint rejected the client credentials (HTTP {status})"
            ),
        });
    }
    Error::unavailable(format!(
        "the OAuth2 token endpoint is unavailable (HTTP {status})"
    ))
}

/// Transport failures are transient. The underlying message is never rendered:
/// it can embed request URLs and header values.
fn transport_error(error: HttpClientError) -> Error {
    match error {
        HttpClientError::Timeout(_) => Error::unavailable("the OAuth2 token request timed out"),
        HttpClientError::Connect(_) => {
            Error::unavailable("the OAuth2 token endpoint is unreachable")
        }
        _ => Error::unavailable("the OAuth2 token request could not be completed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_are_normalized_to_one_order_independent_set() {
        assert_eq!(
            normalize_scopes([" write", "read", "write ", ""]),
            vec!["read".to_string(), "write".to_string()]
        );
        assert!(normalize_scopes(Vec::<&str>::new()).is_empty());
    }

    #[test]
    fn the_form_body_is_encoded_and_keeps_parameter_order() {
        let form = form_encode(&[
            ("grant_type".to_string(), "client_credentials".to_string()),
            ("scope".to_string(), "read write".to_string()),
            (
                "audience".to_string(),
                "https://api.example.com".to_string(),
            ),
        ]);
        assert_eq!(
            form,
            "grant_type=client_credentials&scope=read+write&audience=https%3A%2F%2Fapi.example.com"
        );
    }

    #[test]
    fn basic_credentials_form_encode_both_halves_before_base64() {
        // A `:` in the secret must not look like the id/secret separator.
        let credentials = basic_credentials("svc client", "s3cret:value");
        assert_eq!(credentials, "c3ZjK2NsaWVudDpzM2NyZXQlM0F2YWx1ZQ==");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&credentials)
                .expect("base64"),
            b"svc+client:s3cret%3Avalue"
        );
    }

    #[test]
    fn the_configured_defaults_are_the_documented_ones() {
        let config =
            ClientCredentialsConfig::new("https://id.example.com/token", "id", "s", ["read"]);
        assert_eq!(config.refresh_ahead, Duration::from_secs(60));
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert_eq!(config.client_auth, ClientSecretAuth::Basic);
        assert!(!config.allow_private_networks);
        assert_eq!(config.scopes, vec!["read".to_string()]);
    }
}
