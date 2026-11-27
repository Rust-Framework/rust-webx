//! External PDP authorization client over HTTP (F4).
//!
//! [`HttpAuthorizationClient`] implements [`IAuthorizationClient`] by
//! `POST`ing JSON checks to a policy decision point (PDP) through the framework
//! [`IHttpClient`]. It adds the four things a network authorization dependency
//! needs and the port alone does not provide:
//!
//! * **an explicit wire contract** — one object per check, `{"decision":
//!   "allow"}` / `{"decision": "deny"}` back, case-insensitive. Any other shape
//!   is *malformed*, never guessed: `{"allowed": true}` is not an `Allow`.
//! * **fail-closed failures** — non-2xx, transport errors, timeouts, oversized
//!   bodies and malformed JSON are all [`Error::unavailable`] (`503`,
//!   retryable). There is no degraded path that returns `Allow` without a
//!   positive answer from the PDP.
//! * **a bounded decision cache** — optional TTL, **`Allow` only**: a `Deny` is
//!   never cached, so revoking a permission takes effect on the next check. The
//!   cache keys every field of the check (ordered context, length-prefixed
//!   fields) and evicts the oldest entry (FIFO) when full.
//! * **a DI entry point** — [`HostBuilder::use_authorization_client`] registers
//!   the client as the one `Arc<dyn IAuthorizationClient>` singleton.
//!
//! [`IHttpClient`] is the only transport seam: there is no second port. A test
//! (or an egress-policy wrapper) implements that one trait, records
//! [`OutboundRequest`] through its read accessors and answers with
//! [`OutboundResponse::from_bytes`](webx_client::OutboundResponse::from_bytes).
//!
//! ```ignore
//! let config = AuthorizationHttpConfig::new("https://pdp.example.com/v1/check")
//!     .with_bearer_token(pdp_api_token)
//!     .with_cache_ttl(Duration::from_secs(30));
//! let client = Arc::new(HttpAuthorizationClient::new(config, http, clock)?);
//!
//! Host::builder()
//!     .use_authentication()
//!     .use_authorization_client(client)
//!     .build();
//! ```
//!
//! Timeout, batch size, cache size and TTL are clamped to documented bounds in
//! [`HttpAuthorizationClient::new`], so a configuration typo cannot remove the
//! deadline (0s) or create an unbounded cache. The deadline bounds the **whole**
//! exchange — response headers, the bounded body read and the JSON parse — not
//! just the transport call, so a PDP cannot extend a check by slow-dripping its
//! body.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use webx_client::{HttpClientError, IHttpClient, OutboundRequest};
use webx_core::authorization::{AccessCheck, AccessDecision, IAuthorizationClient};
use webx_core::error::{Error, Result};
use webx_core::time::IClock;

use crate::server::HostBuilder;

/// Default deadline for one PDP request.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// A PDP check sits on the request path; it must never hang longer than this.
const MAX_TIMEOUT: Duration = Duration::from_secs(10);
/// Below this a cold TLS handshake cannot complete, so every check would fail.
const MIN_TIMEOUT: Duration = Duration::from_millis(100);

/// Default maximum number of checks in one batch request.
const DEFAULT_MAX_BATCH: usize = 50;
/// Upper bound for one batch request (policy endpoints cap payloads too).
const MAX_MAX_BATCH: usize = 500;

/// Default upper bound of cached decisions.
const DEFAULT_MAX_CACHE_ENTRIES: usize = 1024;
/// Upper bound of cached decisions (process memory, not a shared cache).
const MAX_MAX_CACHE_ENTRIES: usize = 65_536;

/// Lower bound of a cache TTL: below this the cache is useless churn.
const MIN_CACHE_TTL: Duration = Duration::from_secs(1);
/// Upper bound of a cache TTL: a permission revocation must be visible in
/// bounded time even when nothing calls [`HttpAuthorizationClient::invalidate`].
const MAX_CACHE_TTL: Duration = Duration::from_secs(300);

/// A decision document is tiny; anything larger is a malformed/hostile answer.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// Separator byte (unit separator) in canonical cache keys.
const KEY_SEPARATOR: char = '\u{1f}';

/// Settings for one HTTP PDP.
///
/// [`AuthorizationHttpConfig::new`] fills every value with the documented
/// default; the `with_*` builders change one knob at a time. Values are clamped
/// to their bounds when the client is built (see
/// [`HttpAuthorizationClient::new`]), not when they are set.
#[derive(Clone)]
pub struct AuthorizationHttpConfig {
    /// Absolute decision endpoint. Must be `https://` unless
    /// [`allow_private_networks`](Self::allow_private_networks) is enabled.
    pub endpoint: String,
    /// End-to-end deadline for one decision exchange (headers, body and parse),
    /// clamped to `100ms..=10s`.
    pub timeout: Duration,
    /// Maximum checks in one batch request, clamped to `1..=500`. A larger
    /// [`IAuthorizationClient::check_batch`] is a configuration error.
    pub max_batch: usize,
    /// Optional PDP bearer credential. Never rendered by `Debug`.
    pub bearer_token: Option<String>,
    /// Decision cache lifetime. `None` (default) disables caching; `Some` is
    /// clamped to `1s..=300s`, and only `Allow` decisions are cached.
    pub cache_ttl: Option<Duration>,
    /// Upper bound of cached decisions, clamped to `1..=65536`.
    pub max_cache_entries: usize,
    /// Allow a plaintext/private endpoint (tests, intranet PDP).
    pub allow_private_networks: bool,
}

impl Default for AuthorizationHttpConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            timeout: DEFAULT_TIMEOUT,
            max_batch: DEFAULT_MAX_BATCH,
            bearer_token: None,
            cache_ttl: None,
            max_cache_entries: DEFAULT_MAX_CACHE_ENTRIES,
            allow_private_networks: false,
        }
    }
}

impl fmt::Debug for AuthorizationHttpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationHttpConfig")
            .field("endpoint", &self.endpoint)
            .field("timeout", &self.timeout)
            .field("max_batch", &self.max_batch)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("cache_ttl", &self.cache_ttl)
            .field("max_cache_entries", &self.max_cache_entries)
            .field("allow_private_networks", &self.allow_private_networks)
            .finish()
    }
}

impl AuthorizationHttpConfig {
    /// Configuration for `endpoint` with framework defaults.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            ..Self::default()
        }
    }

    /// Set the per-request deadline (clamped to `100ms..=10s`).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the maximum batch size (clamped to `1..=500`).
    pub fn with_max_batch(mut self, max_batch: usize) -> Self {
        self.max_batch = max_batch;
        self
    }

    /// Send `Authorization: Bearer <token>` with every decision request.
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    /// Cache `Allow` decisions for `ttl` (clamped to `1s..=300s`).
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Disable the decision cache (the default).
    pub fn without_cache(mut self) -> Self {
        self.cache_ttl = None;
        self
    }

    /// Set the cache entry bound (clamped to `1..=65536`).
    pub fn with_max_cache_entries(mut self, entries: usize) -> Self {
        self.max_cache_entries = entries;
        self
    }

    /// Allow a plaintext endpoint (tests, intranet PDP).
    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }
}

/// An external PDP client: JSON over HTTP, fail-closed, optional decision cache.
///
/// Build it with [`HttpAuthorizationClient::new`] and register it with
/// [`HostBuilder::use_authorization_client`]; business code injects
/// `Arc<dyn IAuthorizationClient>`.
pub struct HttpAuthorizationClient {
    config: AuthorizationHttpConfig,
    http: Arc<dyn IHttpClient>,
    clock: Arc<dyn IClock>,
    cache: Mutex<DecisionCache>,
}

impl fmt::Debug for HttpAuthorizationClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Response bodies and decision payloads are never rendered: they can
        // carry internal subject and resource data.
        formatter
            .debug_struct("HttpAuthorizationClient")
            .field("config", &self.config)
            .finish()
    }
}

impl HttpAuthorizationClient {
    /// Build a client over the framework HTTP client.
    ///
    /// The endpoint is validated and every tunable is clamped here, so a
    /// misconfigured service fails at start-up instead of on its first check:
    ///
    /// | field | bound |
    /// |---|---|
    /// | `timeout` | `100ms..=10s` |
    /// | `max_batch` | `1..=500` |
    /// | `cache_ttl` | `Some(1s..=300s)` |
    /// | `max_cache_entries` | `1..=65536` |
    ///
    /// Errors are [`Error::di`] (`500`): they are configuration defects, not a
    /// PDP outage.
    ///
    /// `config.allow_private_networks` gates this client's own `https://` check
    /// only; the injected `IHttpClient` keeps its own SSRF policy.
    pub fn new(
        config: AuthorizationHttpConfig,
        http: Arc<dyn IHttpClient>,
        clock: Arc<dyn IClock>,
    ) -> Result<Self> {
        let config = normalize_config(config)?;
        let cache = DecisionCache::new(
            config.cache_ttl.map(|ttl| ttl.as_secs()),
            config.max_cache_entries,
        );
        Ok(Self {
            config,
            http,
            clock,
            cache: Mutex::new(cache),
        })
    }

    /// The effective configuration (after validation and clamping).
    pub fn config(&self) -> &AuthorizationHttpConfig {
        &self.config
    }

    /// Drop every cached decision.
    ///
    /// Call this after a role or permission change for a subject that must not
    /// wait for its TTL; `invalidate` is global because the cache is per client.
    pub fn invalidate(&self) {
        self.cache_guard().clear();
    }

    fn cache_guard(&self) -> std::sync::MutexGuard<'_, DecisionCache> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `true` when an unexpired cached `Allow` covers `key`.
    fn cache_hit(&self, key: &str) -> bool {
        let now = self.clock.now_unix();
        self.cache_guard().take_allow(key, now)
    }

    /// Cache an `Allow` decision, evicting the oldest entry when full.
    fn cache_allow(&self, key: String) {
        let now = self.clock.now_unix();
        self.cache_guard().insert_allow(key, now);
    }

    /// Send one JSON body to the PDP and return the raw `(status, body)`.
    ///
    /// No deadline is applied here: [`Self::deadline`] wraps this call **and**
    /// the caller's JSON parse in one `tokio::time::timeout`. The timeout below
    /// is only a transport hint ([`OutboundRequest::timeout`]), because an
    /// [`IHttpClient`] may ignore it — and even a client that honours it bounds
    /// the response *headers*, not a body that trickles in afterwards.
    async fn post(&self, body: &[u8]) -> Result<(u16, Vec<u8>)> {
        let timeout = self.config.timeout;
        let mut request = OutboundRequest::post(self.config.endpoint.as_str())
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(body.to_vec())
            .timeout(timeout);
        if let Some(token) = &self.config.bearer_token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self.http.execute(request).await.map_err(transport_error)?;
        let status = response.status;
        // Read (and bound) the body before the status check: an oversized answer
        // is a malformed answer whether or not the status is a success.
        let body = response
            .body
            .collect(MAX_BODY_BYTES)
            .await
            .map_err(transport_error)?;
        if !(200..300).contains(&status) {
            return Err(status_error(status));
        }
        Ok((status, body.to_vec()))
    }

    /// Run one PDP exchange under the configured end-to-end deadline.
    ///
    /// The timeout wraps the transport call **and** everything that follows it
    /// (body collection, size limit, JSON parse). A PDP that answers with
    /// headers immediately and then drips the body is therefore bounded by
    /// `config.timeout` too, not just `execute`. A timeout is a retryable `503`
    /// and never carries the URL or the response body.
    async fn deadline<T>(&self, exchange: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::time::timeout(self.config.timeout, exchange)
            .await
            .map_err(|_| Error::unavailable("the authorization PDP request timed out"))?
    }

    async fn request_single(&self, check: &AccessCheck) -> Result<AccessDecision> {
        let body = encode_single(check)?;
        self.deadline(async {
            let (_, body) = self.post(&body).await?;
            parse_single(&body)
        })
        .await
    }

    async fn request_batch(&self, checks: &[&AccessCheck]) -> Result<Vec<AccessDecision>> {
        let body = encode_batch(checks)?;
        self.deadline(async {
            let (_, body) = self.post(&body).await?;
            parse_batch(&body, checks.len())
        })
        .await
    }
}

#[async_trait]
impl IAuthorizationClient for HttpAuthorizationClient {
    async fn check(&self, check: &AccessCheck) -> Result<AccessDecision> {
        if self.config.cache_ttl.is_some() {
            let key = cache_key(check);
            if self.cache_hit(&key) {
                return Ok(AccessDecision::Allow);
            }
            let decision = self.request_single(check).await?;
            if decision == AccessDecision::Allow {
                self.cache_allow(key);
            }
            return Ok(decision);
        }
        self.request_single(check).await
    }

    async fn check_batch(&self, checks: &[AccessCheck]) -> Result<Vec<AccessDecision>> {
        if checks.is_empty() {
            return Ok(Vec::new());
        }
        if checks.len() > self.config.max_batch {
            return Err(Error::di(format!(
                "authorization batch of {} checks exceeds the configured max_batch of {}",
                checks.len(),
                self.config.max_batch
            )));
        }

        if self.config.cache_ttl.is_none() {
            let refs: Vec<&AccessCheck> = checks.iter().collect();
            return self.request_batch(&refs).await;
        }

        // Serve cached `Allow`s locally; ask the PDP only about the misses, then
        // merge back in the caller's order.
        let mut decisions: Vec<Option<AccessDecision>> = vec![None; checks.len()];
        let mut misses: Vec<(usize, &AccessCheck, String)> = Vec::new();
        for (index, check) in checks.iter().enumerate() {
            let key = cache_key(check);
            if self.cache_hit(&key) {
                decisions[index] = Some(AccessDecision::Allow);
            } else {
                misses.push((index, check, key));
            }
        }
        if misses.is_empty() {
            return Ok(decisions
                .into_iter()
                .map(|decision| decision.unwrap_or(AccessDecision::Deny))
                .collect());
        }

        let refs: Vec<&AccessCheck> = misses.iter().map(|(_, check, _)| *check).collect();
        let returned = self.request_batch(&refs).await?;
        for ((index, _, key), decision) in misses.into_iter().zip(returned) {
            if decision == AccessDecision::Allow {
                self.cache_allow(key);
            }
            decisions[index] = Some(decision);
        }
        Ok(decisions
            .into_iter()
            .map(|decision| decision.unwrap_or(AccessDecision::Deny))
            .collect())
    }
}

/// Register the external PDP port (DI singleton).
///
/// Business code injects `Arc<dyn IAuthorizationClient>`; the host does not
/// call it automatically — a route opts in through `#[authorize(dynamic)]` plus
/// an `IDynamicAuthorizer`, or handler code calls `check` directly.
impl HostBuilder {
    /// Register this host's [`IAuthorizationClient`].
    pub fn use_authorization_client(self, client: Arc<dyn IAuthorizationClient>) -> Self {
        self.register(move |services| services.instance::<dyn IAuthorizationClient>(client))
    }
}

// ---------------------------------------------------------------- wire format

/// The single-check and batch-element JSON shape.
///
/// Field order is the documented contract: `subject`, `tenant_id`, `permission`,
/// `resource`, `context`. Absent optionals stay present as `null`; an empty
/// context stays `{}`.
#[derive(Serialize)]
struct WireCheck<'a> {
    subject: &'a str,
    tenant_id: Option<&'a str>,
    permission: &'a str,
    resource: Option<&'a str>,
    context: &'a BTreeMap<String, String>,
}

impl<'a> From<&'a AccessCheck> for WireCheck<'a> {
    fn from(check: &'a AccessCheck) -> Self {
        Self {
            subject: &check.subject,
            tenant_id: check.tenant_id.as_deref(),
            permission: &check.permission,
            resource: check.resource.as_deref(),
            context: &check.context,
        }
    }
}

fn encode_single(check: &AccessCheck) -> Result<Vec<u8>> {
    serde_json::to_vec(&WireCheck::from(check))
        .map_err(|error| Error::internal(format!("cannot encode an authorization check: {error}")))
}

fn encode_batch(checks: &[&AccessCheck]) -> Result<Vec<u8>> {
    let wire: Vec<WireCheck<'_>> = checks.iter().map(|check| WireCheck::from(*check)).collect();
    serde_json::to_vec(&wire)
        .map_err(|error| Error::internal(format!("cannot encode an authorization batch: {error}")))
}

// ------------------------------------------------------------- wire response

fn malformed(detail: &str) -> Error {
    Error::unavailable(format!(
        "the authorization PDP returned a malformed decision: {detail}"
    ))
}

fn status_error(status: u16) -> Error {
    // Includes 401/403 from the PDP: a rejected *client credential* is still an
    // outage for this caller, and must never become an `Allow`.
    Error::unavailable(format!(
        "the authorization PDP is unavailable (HTTP {status})"
    ))
}

/// Transport failures are transient. The underlying message is never rendered:
/// it can embed request URLs and header values.
fn transport_error(error: HttpClientError) -> Error {
    match error {
        HttpClientError::Timeout(_) => {
            Error::unavailable("the authorization PDP request timed out")
        }
        HttpClientError::Connect(_) => Error::unavailable("the authorization PDP is unreachable"),
        HttpClientError::BlockedAddress(_) => {
            Error::unavailable("the authorization PDP address is blocked by the outbound guard")
        }
        HttpClientError::BodyTooLarge { .. } => Error::unavailable(format!(
            "the authorization PDP response exceeded {MAX_BODY_BYTES} bytes"
        )),
        _ => Error::unavailable("the authorization PDP request could not be completed"),
    }
}

/// Accept only `{"decision": "allow"}` / `{"decision": "deny"}`.
///
/// Anything else — `{"allowed": true}`, an empty body, a bare string — is
/// malformed. Guessing here would turn a schema drift into an authorization
/// bypass.
fn parse_single(body: &[u8]) -> Result<AccessDecision> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| malformed("the response body is not JSON"))?;
    decision_of(&value)
}

fn decision_of(value: &Value) -> Result<AccessDecision> {
    let decision = value
        .get("decision")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("expected a `decision` string"))?;
    if decision.eq_ignore_ascii_case("allow") {
        Ok(AccessDecision::Allow)
    } else if decision.eq_ignore_ascii_case("deny") {
        Ok(AccessDecision::Deny)
    } else {
        Err(malformed("`decision` must be `allow` or `deny`"))
    }
}

/// The batch answer must be an array of exactly `expected` decision objects.
fn parse_batch(body: &[u8], expected: usize) -> Result<Vec<AccessDecision>> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| malformed("the response body is not JSON"))?;
    let Value::Array(items) = value else {
        return Err(malformed("a batch answer must be a JSON array"));
    };
    if items.len() != expected {
        return Err(Error::unavailable(format!(
            "the authorization PDP returned {} decisions for {expected} checks",
            items.len()
        )));
    }
    items.iter().map(decision_of).collect()
}

// ------------------------------------------------------------------- caching

/// Bounded FIFO cache of `Allow` decisions.
///
/// `Deny` is never stored: revocation must take effect, and a negative cache is
/// the wrong default for an authorization decision.
struct DecisionCache {
    /// `None` disables the cache entirely.
    ttl_secs: Option<u64>,
    max_entries: usize,
    entries: HashMap<String, CacheEntry>,
    /// Insertion order, guarded by the entry generation so an overwritten key
    /// is not removed by its stale position.
    order: VecDeque<(String, u64)>,
    sequence: u64,
}

struct CacheEntry {
    expires_at_unix: u64,
    sequence: u64,
}

impl DecisionCache {
    fn new(ttl_secs: Option<u64>, max_entries: usize) -> Self {
        Self {
            ttl_secs,
            max_entries,
            entries: HashMap::new(),
            order: VecDeque::new(),
            sequence: 0,
        }
    }

    /// Consume a cached `Allow`, or report a miss after dropping an expired
    /// entry.
    fn take_allow(&mut self, key: &str, now: u64) -> bool {
        if self.ttl_secs.is_none() {
            return false;
        }
        match self.entries.get(key) {
            Some(entry) if now < entry.expires_at_unix => true,
            Some(_) => {
                self.entries.remove(key);
                false
            }
            None => false,
        }
    }

    /// Store an `Allow` decision, evicting the oldest live entry when full.
    fn insert_allow(&mut self, key: String, now: u64) {
        let Some(ttl_secs) = self.ttl_secs else {
            return;
        };
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.entries.insert(
            key.clone(),
            CacheEntry {
                expires_at_unix: now.saturating_add(ttl_secs),
                sequence,
            },
        );
        self.order.push_back((key, sequence));

        // `order` is at least as long as `entries`, so bounding it bounds the
        // cache; stale positions are dropped without touching a live entry.
        while self.order.len() > self.max_entries {
            let Some((oldest, sequence)) = self.order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&oldest)
                .is_some_and(|entry| entry.sequence == sequence)
            {
                self.entries.remove(&oldest);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// Canonical cache identity of a check: every field, length-prefixed with unit
/// separators so `("ab", Some("c"))` cannot collide with `("a", Some("bc"))`.
fn cache_key(check: &AccessCheck) -> String {
    let mut key = String::new();
    push_text(&mut key, &check.subject);
    match check.tenant_id.as_deref() {
        Some(tenant_id) => {
            key.push('1');
            push_text(&mut key, tenant_id);
        }
        None => key.push('0'),
    }
    push_text(&mut key, &check.permission);
    match check.resource.as_deref() {
        Some(resource) => {
            key.push('1');
            push_text(&mut key, resource);
        }
        None => key.push('0'),
    }
    key.push_str(&check.context.len().to_string());
    key.push(KEY_SEPARATOR);
    for (name, value) in &check.context {
        push_text(&mut key, name);
        push_text(&mut key, value);
    }
    key
}

fn push_text(key: &mut String, value: &str) {
    key.push_str(&value.len().to_string());
    key.push(KEY_SEPARATOR);
    key.push_str(value);
    key.push(KEY_SEPARATOR);
}

// ------------------------------------------------------------------ validation

/// Validate the endpoint and clamp every tunable to its documented bounds.
fn normalize_config(mut config: AuthorizationHttpConfig) -> Result<AuthorizationHttpConfig> {
    config.endpoint = config.endpoint.trim().to_string();
    if config.endpoint.is_empty() {
        return Err(Error::di(
            "authorization PDP endpoint must not be empty".to_string(),
        ));
    }
    let uri: hyper::Uri = config.endpoint.parse().map_err(|error| {
        Error::di(format!(
            "authorization PDP endpoint is not an absolute URL: {error}"
        ))
    })?;
    if uri.scheme().is_none() || uri.authority().is_none() {
        return Err(Error::di(format!(
            "authorization PDP endpoint must be an absolute URL (scheme + host): {}",
            config.endpoint
        )));
    }
    match uri.scheme_str() {
        Some("https") => {}
        Some("http") if config.allow_private_networks => {}
        Some("http") => return Err(Error::di(
            "authorization PDP endpoint must be https:// unless allow_private_networks is enabled"
                .to_string(),
        )),
        Some(scheme) => {
            return Err(Error::di(format!(
                "authorization PDP endpoint must use https:// (got scheme '{scheme}')"
            )))
        }
        None => {
            return Err(Error::di(
                "authorization PDP endpoint must be an absolute URL (scheme + host)".to_string(),
            ))
        }
    }

    config.timeout = config.timeout.clamp(MIN_TIMEOUT, MAX_TIMEOUT);
    config.max_batch = config.max_batch.clamp(1, MAX_MAX_BATCH);
    config.max_cache_entries = config.max_cache_entries.clamp(1, MAX_MAX_CACHE_ENTRIES);
    config.cache_ttl = config
        .cache_ttl
        .map(|ttl| ttl.clamp(MIN_CACHE_TTL, MAX_CACHE_TTL));
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_documented_ones() {
        let config = AuthorizationHttpConfig::new("https://pdp.example.com/check");
        assert_eq!(config.timeout, Duration::from_secs(2));
        assert_eq!(config.max_batch, 50);
        assert_eq!(config.cache_ttl, None);
        assert_eq!(config.max_cache_entries, 1024);
        assert!(!config.allow_private_networks);
        assert!(config.bearer_token.is_none());
    }

    #[test]
    fn debug_redacts_the_bearer_token() {
        let config = AuthorizationHttpConfig::new("https://pdp.example.com/check")
            .with_bearer_token("pdp-secret-token");
        let rendered = format!("{config:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("pdp-secret-token"));
    }

    #[test]
    fn cache_keys_are_unambiguous() {
        let mut first = AccessCheck {
            subject: "ab".to_string(),
            tenant_id: Some("c".to_string()),
            permission: "read".to_string(),
            ..AccessCheck::default()
        };
        let mut second = AccessCheck {
            subject: "a".to_string(),
            tenant_id: Some("bc".to_string()),
            permission: "read".to_string(),
            ..AccessCheck::default()
        };
        assert_ne!(cache_key(&first), cache_key(&second));

        first.context.insert("k".to_string(), "v".to_string());
        second.context.insert("k".to_string(), "v".to_string());
        assert_ne!(cache_key(&first), cache_key(&second));
        assert_eq!(cache_key(&first), cache_key(&first.clone()));

        let unscoped = AccessCheck {
            tenant_id: None,
            ..first.clone()
        };
        assert_ne!(
            cache_key(&unscoped),
            cache_key(&first),
            "`None` is a different question from an empty tenant"
        );
    }

    #[test]
    fn config_clamping_keeps_the_deadline_and_the_cache_bounded() {
        let config = normalize_config(
            AuthorizationHttpConfig::new("https://pdp.example.com/check")
                .with_timeout(Duration::ZERO)
                .with_max_batch(0)
                .with_max_cache_entries(0)
                .with_cache_ttl(Duration::from_secs(9_999)),
        )
        .expect("valid endpoint");
        assert_eq!(config.timeout, MIN_TIMEOUT);
        assert_eq!(config.max_batch, 1);
        assert_eq!(config.max_cache_entries, 1);
        assert_eq!(config.cache_ttl, Some(MAX_CACHE_TTL));
    }

    #[test]
    fn plaintext_endpoints_need_the_explicit_opt_in() {
        let error = normalize_config(AuthorizationHttpConfig::new("http://127.0.0.1:8080/check"))
            .expect_err("plaintext is rejected by default");
        assert_eq!(error.status_code(), 500);
        assert_eq!(error.code(), "dependency_injection");

        let config = AuthorizationHttpConfig::new("http://127.0.0.1:8080/check")
            .allow_private_networks(true);
        assert!(normalize_config(config).is_ok());
    }

    #[test]
    fn the_wire_shape_keeps_null_optionals_and_an_empty_context() {
        let body = encode_single(&AccessCheck {
            subject: "alice".to_string(),
            permission: "orders.read".to_string(),
            ..AccessCheck::default()
        })
        .expect("encodes");
        assert_eq!(
            String::from_utf8(body).expect("UTF-8"),
            r#"{"subject":"alice","tenant_id":null,"permission":"orders.read","resource":null,"context":{}}"#
        );
    }

    #[test]
    fn unknown_decision_shapes_are_malformed() {
        for body in [
            br#"{"allowed":true}"#.as_slice(),
            br#"{"decision":"maybe"}"#.as_slice(),
            br#"{"decision":true}"#.as_slice(),
            b"not json".as_slice(),
            b"".as_slice(),
        ] {
            let error = parse_single(body).expect_err("malformed answers must fail closed");
            assert_eq!(error.status_code(), 503);
            assert!(error.retryable());
        }
        assert_eq!(
            parse_single(br#"{"decision":"ALLOW"}"#).expect("case-insensitive"),
            AccessDecision::Allow
        );
        assert_eq!(
            parse_single(br#"{"decision":"Deny"}"#).expect("case-insensitive"),
            AccessDecision::Deny
        );
        assert!(parse_single(br#"{"decision":"allow","extra":1}"#).is_ok());
    }

    #[test]
    fn batch_answers_must_match_the_request_length() {
        let error = parse_batch(br#"[{"decision":"allow"}]"#, 2)
            .expect_err("a short batch answer is malformed");
        assert_eq!(error.status_code(), 503);
        assert_eq!(
            parse_batch(br#"[{"decision":"allow"},{"decision":"deny"}]"#, 2)
                .expect("ordered batch"),
            vec![AccessDecision::Allow, AccessDecision::Deny]
        );
    }
}
