//! Request-level idempotency: in-memory store + `Idempotency-Key` middleware.
//!
//! [`InMemoryIdempotencyStore`] is the single-process implementation of
//! [`IIdempotencyStore`] (bounded key count, TTL-swept on access, per-attempt
//! fencing tokens). [`IdempotencyMiddleware`] is a continuation-style
//! middleware: it reserves the key, runs the rest of the pipeline at most once,
//! and stores or releases the response.
//!
//! ```text
//! POST /orders            Idempotency-Key: 3f9c…
//!   → first request:   handler runs, 201 stored
//!   → retry (same key): 201 replayed, `Idempotency-Replayed: true`
//!   → concurrent duplicate: 409 + Retry-After
//! ```
//!
//! Deployment semantics: the in-memory store is per process, so multi-instance
//! deployments must register a shared store (Redis/DB) through
//! `HostBuilder::use_idempotency`; the middleware only speaks the port.
//!
//! The store issues a fencing token per reservation (`begin_lease`) and passes
//! it back on `complete_lease`/`release_lease`, so a request whose reservation
//! expired (and was reclaimed by a retry) cannot overwrite the retry's
//! response. The lease passed to the store is separate from the replay retention
//! (`Idempotency.LeaseSeconds`, capped at `Idempotency.TtlSeconds`): a crashed
//! handler becomes retryable after the short lease, while a stored response
//! replays for the whole retention period. The middleware also captures the
//! replayable response headers ([`REPLAYABLE_RESPONSE_HEADERS`]) so a replay is
//! byte- and header-identical.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::idempotency::{
    IIdempotencyStore, IdempotencyBegin, IdempotencyLease, IdempotentResponse, DEFAULT_LEASE,
    IDEMPOTENCY_KEY_HEADER, IDEMPOTENCY_REPLAYED_HEADER, REPLAYABLE_RESPONSE_HEADERS,
};
use webx_core::middleware::{IMiddleware, NextStage};

use crate::problem_response::write_problem;

struct Entry {
    expires_at: Instant,
    /// `None` while the request that reserved the key is still running.
    response: Option<IdempotentResponse>,
    /// Fencing token of the live reservation; cleared when the response is stored.
    attempt: Option<String>,
}

/// Process-local idempotency store.
///
/// Bounded by `max_keys`; expired entries are pruned on access, so no
/// background task is needed. When a new reservation would exceed the bound,
/// eviction prefers a completed entry (its handler already ran), then an
/// expired entry, and only as a last resort an in-progress reservation (which
/// lets the client's retry re-run the handler).
///
/// Single-process semantics: the state (including the fencing tokens) is
/// visible only inside this process; multi-instance deployments need a shared
/// store.
pub struct InMemoryIdempotencyStore {
    max_keys: usize,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for InMemoryIdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIdempotencyStore {
    /// A store bounded to 10,000 keys.
    pub fn new() -> Self {
        Self::with_max_keys(10_000)
    }

    /// A store bounded to `max_keys` (at least 1).
    pub fn with_max_keys(max_keys: usize) -> Self {
        Self {
            max_keys: max_keys.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Number of live keys (expired entries pruned).
    pub fn tracked_keys(&self) -> usize {
        let now = Instant::now();
        let mut entries = self.lock();
        prune(&mut entries, now);
        entries.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Store `response` only if the reservation is still in progress and, when
    /// `expected` is set, is still owned by that attempt token. The completed
    /// entry replays for `retention`.
    fn complete_if(
        &self,
        key: &str,
        expected: Option<&str>,
        response: IdempotentResponse,
        retention: Duration,
    ) {
        let now = Instant::now();
        let mut entries = self.lock();
        prune(&mut entries, now);
        let Some(entry) = entries.get_mut(key) else {
            return;
        };
        // A completed key keeps its response; only the live reservation may
        // transition to completed.
        if entry.response.is_some() {
            return;
        }
        if let Some(expected) = expected {
            if entry.attempt.as_deref() != Some(expected) {
                // A stale attempt (the reservation expired and was reclaimed):
                // never overwrite the new owner's state.
                return;
            }
        }
        entry.response = Some(response);
        entry.attempt = None;
        entry.expires_at = now + retention;
    }

    /// Remove the reservation only if it is still in progress and, when
    /// `expected` is set, is still owned by that attempt token.
    fn release_if(&self, key: &str, expected: Option<&str>) {
        let now = Instant::now();
        let mut entries = self.lock();
        prune(&mut entries, now);
        let removable = entries.get(key).is_some_and(|entry| {
            entry.response.is_none()
                && expected.is_none_or(|token| entry.attempt.as_deref() == Some(token))
        });
        if removable {
            entries.remove(key);
        }
    }
}

fn prune(entries: &mut HashMap<String, Entry>, now: Instant) {
    entries.retain(|_, entry| entry.expires_at > now);
}

/// Evict one entry to make room for a new reservation.
///
/// Priority (each tier evicts the entry with the earliest expiry):
/// 1. completed entries — their handler already ran; dropping the retention
///    early only allows re-execution on the key's next use;
/// 2. expired entries — normally removed by [`prune`] first, kept here so the
///    priority is a property of this function rather than of its callers;
/// 3. in-progress reservations — last resort; dropping one lets the client's
///    retry run the handler instead of holding a key that can never fit.
fn evict_one(entries: &mut HashMap<String, Entry>, now: Instant) {
    let victim = entries
        .iter()
        .filter(|(_, entry)| entry.response.is_some())
        .min_by_key(|(_, entry)| entry.expires_at)
        .or_else(|| {
            entries
                .iter()
                .filter(|(_, entry)| entry.expires_at <= now)
                .min_by_key(|(_, entry)| entry.expires_at)
        })
        .or_else(|| entries.iter().min_by_key(|(_, entry)| entry.expires_at))
        .map(|(key, _)| key.clone());
    if let Some(key) = victim {
        entries.remove(&key);
    }
}

/// A fresh opaque fencing token for one reservation attempt.
fn new_attempt_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Keep only the replayable headers of a cached response, lower-cased.
///
/// The middleware only ever stores the [`REPLAYABLE_RESPONSE_HEADERS`], but a
/// response handed to [`IIdempotencyStore::complete`] by custom code may carry
/// anything. Replay is a security boundary (never replay `Set-Cookie` /
/// `Authorization`), so the in-memory store filters on the way out exactly like
/// the durable store does.
fn filter_replayable_response(mut response: IdempotentResponse) -> IdempotentResponse {
    response.headers.retain(|(name, _)| {
        REPLAYABLE_RESPONSE_HEADERS
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))
    });
    for (name, _) in &mut response.headers {
        name.make_ascii_lowercase();
    }
    response
}

#[async_trait]
impl IIdempotencyStore for InMemoryIdempotencyStore {
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin> {
        // Legacy token-less entry point: the reservation and the stored response
        // are both bounded by `ttl` (no separate lease).
        let (begin, _lease) = self.begin_lease(key, ttl, ttl).await?;
        Ok(begin)
    }

    async fn begin_lease(
        &self,
        key: &str,
        _retention: Duration,
        lease: Duration,
    ) -> Result<(IdempotencyBegin, Option<IdempotencyLease>)> {
        let now = Instant::now();
        let mut entries = self.lock();
        // A live reservation answers immediately; an expired one is reclaimed by
        // replacing its stale entry below (so it never forces a live entry out).
        if entries.get(key).is_some_and(|entry| entry.expires_at > now) {
            let entry = entries.get(key).expect("checked above");
            return Ok(match &entry.response {
                Some(response) => (
                    IdempotencyBegin::Replay(filter_replayable_response(response.clone())),
                    None,
                ),
                None => (IdempotencyBegin::InProgress, None),
            });
        }
        entries.remove(key);
        while entries.len() >= self.max_keys {
            evict_one(&mut entries, now);
        }
        let attempt = new_attempt_token();
        entries.insert(
            key.to_string(),
            Entry {
                // The reservation lives for the lease, not for the replay
                // retention: a crashed handler becomes retryable after `lease`.
                expires_at: now + lease,
                response: None,
                attempt: Some(attempt.clone()),
            },
        );
        Ok((
            IdempotencyBegin::Started,
            Some(IdempotencyLease::new(attempt)),
        ))
    }

    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()> {
        self.complete_lease(key, None, response, ttl).await
    }

    async fn complete_lease(
        &self,
        key: &str,
        lease: Option<&IdempotencyLease>,
        response: IdempotentResponse,
        retention: Duration,
    ) -> Result<()> {
        self.complete_if(
            key,
            lease.map(|lease| lease.token.as_str()),
            response,
            retention,
        );
        Ok(())
    }

    async fn release(&self, key: &str) -> Result<()> {
        self.release_if(key, None);
        Ok(())
    }

    async fn release_lease(&self, key: &str, lease: Option<&IdempotencyLease>) -> Result<()> {
        self.release_if(key, lease.map(|lease| lease.token.as_str()));
        Ok(())
    }
}

/// Continuation-style idempotency middleware.
///
/// Runs after authentication (so the principal can scope the key) and reserves
/// the key before the rest of the pipeline executes.
pub struct IdempotencyMiddleware {
    store: std::sync::Arc<dyn IIdempotencyStore>,
    ttl: Duration,
    lease: Duration,
    max_body_bytes: usize,
    allow_anonymous: bool,
    /// Only these path prefixes are idempotent; empty = every path.
    paths: Vec<String>,
    /// Path prefixes that are never idempotent (wins over `paths`).
    skip_paths: Vec<String>,
}

impl IdempotencyMiddleware {
    /// Middleware over `store` with a per-key TTL (default body cap: 1 MiB).
    ///
    /// The reservation lease defaults to [`DEFAULT_LEASE`] (60s) and can be
    /// changed with [`Self::with_lease`]; the host wires it from
    /// `Idempotency.LeaseSeconds`.
    pub fn new(store: std::sync::Arc<dyn IIdempotencyStore>, ttl: Duration) -> Self {
        Self {
            store,
            ttl,
            lease: DEFAULT_LEASE,
            max_body_bytes: 1024 * 1024,
            allow_anonymous: true,
            paths: Vec::new(),
            skip_paths: Vec::new(),
        }
    }

    /// Allow or reject idempotency for anonymous requests (default: allow).
    ///
    /// Anonymous requests are scoped by the resolved client IP; when the
    /// client's `Idempotency-Key` is derived deterministically from the request
    /// body, two different clients behind one NAT/proxy could replay each
    /// other's response. Set this to `false` unless every anonymous
    /// state-changing endpoint is safe to replay across clients.
    pub fn with_anonymous_allowed(mut self, allow_anonymous: bool) -> Self {
        self.allow_anonymous = allow_anonymous;
        self
    }

    /// Change the largest response body that is cached (larger ones release the key).
    pub fn with_max_body_bytes(mut self, max_body_bytes: usize) -> Self {
        self.max_body_bytes = max_body_bytes.max(1);
        self
    }

    /// Change the reservation lease (default [`DEFAULT_LEASE`], 60s).
    ///
    /// A reservation blocks concurrent retries for this long; the middleware
    /// caps it at the replay retention TTL, so `lease <= ttl` always reaches the
    /// store. Keep it above the handler's worst-case duration: a handler that
    /// outlives its lease can be reclaimed and executed a second time.
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// Restrict idempotency to `paths` and exclude `skip_paths`.
    ///
    /// Entries are path prefixes matched on segment boundaries (`/api/orders`
    /// covers `/api/orders` and `/api/orders/123`, not `/api/orders-archive`);
    /// a trailing slash is ignored and `/` covers every path. An empty `paths`
    /// list covers every path (the pre-2.8 behavior); `skip_paths` always wins.
    /// Non-matching requests pass through without reading the key header or
    /// touching the store.
    pub fn with_paths(mut self, paths: Vec<String>, skip_paths: Vec<String>) -> Self {
        self.paths = paths;
        self.skip_paths = skip_paths;
        self
    }

    /// Whether idempotency applies to `path` (skip first, then allow list).
    fn applies_to(&self, path: &str) -> bool {
        if self
            .skip_paths
            .iter()
            .any(|prefix| path_prefix_matches(path, prefix))
        {
            return false;
        }
        self.paths.is_empty()
            || self
                .paths
                .iter()
                .any(|prefix| path_prefix_matches(path, prefix))
    }
}

/// Whether `path` is `prefix` or one of its descendants on a path-segment
/// boundary.
///
/// A trailing slash in `prefix` is ignored, and an empty prefix (or `/`) matches
/// every path.
fn path_prefix_matches(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Capture the replayable headers of a response, in whitelist order.
fn replayable_headers(ctx: &dyn IHttpContext) -> Vec<(String, String)> {
    REPLAYABLE_RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            ctx.response()
                .header(name)
                .map(|value| ((*name).to_string(), value.to_string()))
        })
        .collect()
}

#[async_trait]
impl IMiddleware for IdempotencyMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<std::ops::ControlFlow<()>> {
        unreachable!("IdempotencyMiddleware uses the continuation API")
    }

    fn continuation(&self) -> bool {
        true
    }

    async fn invoke_next(&self, ctx: &mut dyn IHttpContext, next: NextStage) -> Result<()> {
        if !is_state_changing(ctx.request().method()) {
            return next.run(ctx).await;
        }
        // Check the route allow-list before reading/validating the header: a
        // request outside the configured prefixes must not touch the store.
        if !self.applies_to(ctx.request().path()) {
            return next.run(ctx).await;
        }
        let Some(presented) = ctx.request().header(IDEMPOTENCY_KEY_HEADER) else {
            return next.run(ctx).await;
        };
        let presented = presented.trim().to_string();
        if presented.is_empty()
            || presented.len() > 255
            || !presented.bytes().all(|b| b.is_ascii_graphic())
        {
            write_problem(
                ctx,
                400,
                "Idempotency-Key must be 1-255 visible ASCII characters",
            )
            .await;
            return Ok(());
        }
        if !self.allow_anonymous && !is_authenticated(ctx) {
            return next.run(ctx).await;
        }
        let Some(scope) = request_scope(ctx) else {
            return next.run(ctx).await;
        };
        let key = format!("idem:{scope}:{presented}");

        // The store reserves for the short lease (a crashed handler is retryable
        // quickly); the caller's TTL is the replay retention of a completed
        // response. Never hand the store a lease longer than the retention.
        let lease = self.lease.min(self.ttl);
        let (begin, lease) = self.store.begin_lease(&key, self.ttl, lease).await?;
        match begin {
            IdempotencyBegin::Started => {
                let result = next.run(ctx).await;
                let status = ctx.response().status();
                let body = ctx.response().body_bytes();
                let content_type = ctx.response().header("content-type").map(str::to_string);
                let content_encoding = ctx
                    .response()
                    .header("content-encoding")
                    .map(str::to_string);
                let headers = replayable_headers(ctx);
                let stream = content_type
                    .as_deref()
                    .is_some_and(|content_type| content_type.starts_with("text/event-stream"));
                // File/stream responses report `has_body() == true` but expose no
                // bytes; caching them would replay 200 + empty body. A body-less
                // 204/304 is fine to cache (no bytes to lose).
                let bodyless_stream = ctx.response().has_body() && body.is_empty();
                let cacheable = result.is_ok()
                    && (200..300).contains(&status)
                    && !stream
                    && !bodyless_stream
                    && body.len() <= self.max_body_bytes;
                if cacheable {
                    let response =
                        IdempotentResponse::new(status, content_type, content_encoding, body)
                            .with_headers(headers);
                    if let Err(error) = self
                        .store
                        .complete_lease(&key, lease.as_ref(), response, self.ttl)
                        .await
                    {
                        tracing::warn!(%error, "idempotency: storing the response failed");
                        // Drop the reservation so the client can retry instead of
                        // receiving 409 until the TTL expires.
                        match self.store.release_lease(&key, lease.as_ref()).await {
                            Ok(()) => tracing::warn!(
                                "idempotency: reservation released after a store failure"
                            ),
                            Err(release_error) => tracing::error!(
                                %release_error,
                                "idempotency: releasing the reservation after a store failure failed"
                            ),
                        }
                    }
                } else if let Err(error) = self.store.release_lease(&key, lease.as_ref()).await {
                    tracing::warn!(%error, "idempotency: releasing the reservation failed");
                }
                result
            }
            IdempotencyBegin::InProgress => {
                ctx.response_mut().set_header("retry-after", "1");
                write_problem(
                    ctx,
                    409,
                    "a request with this Idempotency-Key is still in progress",
                )
                .await;
                Ok(())
            }
            IdempotencyBegin::Replay(cached) => {
                let response = ctx.response_mut();
                response.set_status(cached.status);
                if let Some(content_type) = cached.content_type {
                    response.set_header("content-type", &content_type);
                }
                if let Some(content_encoding) = cached.content_encoding {
                    response.set_header("content-encoding", &content_encoding);
                }
                for (name, value) in cached.headers {
                    response.set_header(&name, &value);
                }
                response.set_header(IDEMPOTENCY_REPLAYED_HEADER, "true");
                response.write_bytes(cached.body).await
            }
            // `IdempotencyBegin` is `#[non_exhaustive]`; a future variant is a
            // framework bug here (this crate ships with the core crate), so fail
            // closed instead of running the handler without a reservation.
            _ => {
                tracing::error!(
                    "idempotency: the store returned an unknown IdempotencyBegin state; \
                     releasing the reservation and failing the request"
                );
                let _ = self.store.release_lease(&key, lease.as_ref()).await;
                write_problem(ctx, 500, "idempotency state is unavailable").await;
                Ok(())
            }
        }
    }
}

fn is_state_changing(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "POST" | "PUT" | "PATCH" | "DELETE"
    )
}

/// Principal + tenant + method + path + query, so two clients cannot collide
/// on one key and a retry to a different route/query is not replayed.
///
/// The identity comes from the request claims and the client address from
/// `ClientInfo` (the peer address when the `ClientInfo` middleware is not
/// installed), so a deployment behind a reverse proxy scopes anonymous keys by
/// the real client instead of the proxy address.
fn request_scope(ctx: &dyn IHttpContext) -> Option<String> {
    let (subject, tenant) = match ctx.claims() {
        Some(claims) => {
            let subject = claims.subject();
            if subject.is_empty() {
                (None, None)
            } else {
                (Some(subject), claims.get_tenantid())
            }
        }
        None => (None, None),
    };
    let client_ip = ctx
        .client_info()
        .client_ip
        .or_else(|| ctx.request().remote_addr().map(|addr| addr.ip()));
    let query = canonical_query(ctx);
    scope_from_parts(
        subject,
        tenant,
        client_ip,
        ctx.request().method(),
        ctx.request().path(),
        query.as_deref(),
    )
}

/// Whether the request carries an authenticated principal.
fn is_authenticated(ctx: &dyn IHttpContext) -> bool {
    ctx.claims()
        .is_some_and(|claims| !claims.subject().is_empty())
}

/// Canonical query string (`k=v&k2=v2` with sorted keys/values).
///
/// Included in the idempotency scope so `POST /items?mode=a` and
/// `POST /items?mode=b` do not replay each other. Sorting makes the key stable
/// when a client reorders query parameters. Keys and values are escaped so the
/// encoding is injective (see [`escape_scope_part`]).
fn canonical_query(ctx: &dyn IHttpContext) -> Option<String> {
    let pairs: Vec<(String, String)> = match ctx.request().query_all() {
        Some(all) => all
            .iter()
            .flat_map(|(key, values)| values.iter().map(move |value| (key.clone(), value.clone())))
            .collect(),
        None => ctx
            .request()
            .query()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    };
    canonical_query_from_pairs(pairs)
}

/// Canonical form of already-decoded query pairs (pure; unit-tested).
///
/// Sorted by `(key, value)` first, then escaped and joined, so a pair containing
/// `&`/`=` cannot alias another pair list (`{"a": "b&c=d"}` vs
/// `{"a": "b", "c": "d"}` stay distinct).
fn canonical_query_from_pairs(mut pairs: Vec<(String, String)>) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    pairs.sort();
    Some(
        pairs
            .into_iter()
            .map(|(key, value)| {
                format!("{}={}", escape_scope_part(&key), escape_scope_part(&value))
            })
            .collect::<Vec<_>>()
            .join("&"),
    )
}

/// Escape the structural characters of one scope segment.
///
/// The idempotency scope is a `:`/`?`/`&`/`=`-delimited string; escaping those
/// characters (and `%`, the escape character itself) inside every dynamic
/// component makes the encoding **injective**: two different requests can never
/// produce the same key by smuggling separators into a subject, tenant, path or
/// query parameter. Ordinary values (`u-1`, `/api/orders`, `mode=a`) contain
/// none of these characters and stay byte-identical to the pre-2.8 format.
fn escape_scope_part(part: &str) -> String {
    let mut escaped = String::with_capacity(part.len());
    for ch in part.chars() {
        match ch {
            '%' => escaped.push_str("%25"),
            ':' => escaped.push_str("%3A"),
            '?' => escaped.push_str("%3F"),
            '&' => escaped.push_str("%26"),
            '=' => escaped.push_str("%3D"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Build the scope segment from already-resolved request facts.
///
/// The principal is `tenant:<tenant>:user:<sub>` for authenticated
/// tenant-scoped requests, `user:<sub>` for other authenticated requests and
/// `ip:<client>` for anonymous traffic. The tenant segment is a security
/// boundary: the same subject in two tenants must never replay each other's
/// responses (user ids are not guaranteed to be globally unique). Every dynamic
/// component is escaped, so the whole scope is injective.
fn scope_from_parts(
    subject: Option<&str>,
    tenant: Option<&str>,
    client_ip: Option<IpAddr>,
    method: &str,
    path: &str,
    query: Option<&str>,
) -> Option<String> {
    let principal = match subject {
        Some(subject) => match tenant.filter(|tenant| !tenant.is_empty()) {
            Some(tenant) => format!(
                "tenant:{}:user:{}",
                escape_scope_part(tenant),
                escape_scope_part(subject)
            ),
            None => format!("user:{}", escape_scope_part(subject)),
        },
        None => format!("ip:{}", client_ip?),
    };
    let mut scope = format!(
        "{principal}:{}:{}",
        escape_scope_part(&method.to_ascii_uppercase()),
        escape_scope_part(path)
    );
    if let Some(query) = query {
        scope.push('?');
        scope.push_str(query);
    }
    Some(scope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &[u8]) -> IdempotentResponse {
        IdempotentResponse::new(
            status,
            Some("application/json".to_string()),
            None,
            body.to_vec(),
        )
        .with_headers(vec![("etag".to_string(), "\"v1\"".to_string())])
    }

    #[tokio::test]
    async fn started_in_progress_and_replay_follow_the_protocol() {
        let store = InMemoryIdempotencyStore::new();
        let ttl = Duration::from_secs(30);

        assert_eq!(
            store.begin("k", ttl).await.expect("begin"),
            IdempotencyBegin::Started
        );
        assert_eq!(
            store.begin("k", ttl).await.expect("in flight"),
            IdempotencyBegin::InProgress
        );

        store
            .complete("k", response(201, b"{\"id\":1}"), ttl)
            .await
            .expect("complete");
        match store.begin("k", ttl).await.expect("replay") {
            IdempotencyBegin::Replay(stored) => {
                assert_eq!(stored.status, 201);
                assert_eq!(stored.body, b"{\"id\":1}");
                assert_eq!(
                    stored.headers,
                    vec![("etag".to_string(), "\"v1\"".to_string())],
                    "replayable headers round-trip"
                );
            }
            other => panic!("expected a replay, got {other:?}"),
        }
        assert_eq!(store.tracked_keys(), 1);
    }

    #[tokio::test]
    async fn release_frees_the_key_but_a_completed_key_stays() {
        let store = InMemoryIdempotencyStore::new();
        let ttl = Duration::from_secs(30);

        assert_eq!(
            store.begin("k", ttl).await.expect("begin"),
            IdempotencyBegin::Started
        );
        store.release("k").await.expect("release");
        assert_eq!(
            store.begin("k", ttl).await.expect("begin again"),
            IdempotencyBegin::Started
        );

        store
            .complete("k", response(200, b"ok"), ttl)
            .await
            .expect("complete");
        store.release("k").await.expect("release completed");
        assert!(matches!(
            store.begin("k", ttl).await.expect("still stored"),
            IdempotencyBegin::Replay(_)
        ));
    }

    #[tokio::test]
    async fn expired_reservations_are_reclaimable() {
        let store = InMemoryIdempotencyStore::new();
        assert_eq!(
            store
                .begin("k", Duration::from_millis(10))
                .await
                .expect("begin"),
            IdempotencyBegin::Started
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            store
                .begin("k", Duration::from_secs(30))
                .await
                .expect("reclaim"),
            IdempotencyBegin::Started
        );
    }

    #[tokio::test]
    async fn a_stale_lease_cannot_complete_or_release_a_reclaimed_key() {
        let store = InMemoryIdempotencyStore::new();
        let ttl = Duration::from_secs(30);

        let (begin, lease) = store
            .begin_lease("k", ttl, Duration::from_millis(10))
            .await
            .expect("first begin");
        assert_eq!(begin, IdempotencyBegin::Started);
        let old_lease = lease.expect("the store must issue a fencing token");
        tokio::time::sleep(Duration::from_millis(30)).await;

        // The reservation expired; a retry reclaims the key with a new token.
        let (begin, lease) = store.begin_lease("k", ttl, ttl).await.expect("reclaim");
        assert_eq!(begin, IdempotencyBegin::Started);
        let new_lease = lease.expect("token");
        assert_ne!(old_lease.token, new_lease.token);

        // The stale attempt's late completion must not touch the live lease.
        store
            .complete_lease("k", Some(&old_lease), response(201, b"stale"), ttl)
            .await
            .expect("stale completion is silent");
        assert_eq!(
            store.begin("k", ttl).await.expect("still reserved"),
            IdempotencyBegin::InProgress
        );

        // ...and its late release must not delete the live lease either.
        store
            .release_lease("k", Some(&old_lease))
            .await
            .expect("stale release is silent");
        assert_eq!(
            store.begin("k", ttl).await.expect("still reserved"),
            IdempotencyBegin::InProgress
        );

        // The current attempt still completes normally.
        store
            .complete_lease("k", Some(&new_lease), response(202, b"current"), ttl)
            .await
            .expect("current completion");
        match store.begin("k", ttl).await.expect("replay") {
            IdempotencyBegin::Replay(stored) => {
                assert_eq!(stored.status, 202);
                assert_eq!(stored.body, b"current");
            }
            other => panic!("expected the current attempt's response, got {other:?}"),
        }
    }

    /// A replay only ever carries the whitelisted headers, even if a custom
    /// `complete` stored something else (the durable store filters the same way).
    #[tokio::test]
    async fn replayed_headers_are_filtered_to_the_whitelist() {
        let store = InMemoryIdempotencyStore::new();
        let ttl = Duration::from_secs(30);
        let stored = IdempotentResponse::new(201, None, None, b"ok".to_vec()).with_headers(vec![
            ("Set-Cookie".to_string(), "session=secret".to_string()),
            ("ETag".to_string(), "\"v1\"".to_string()),
            ("X-Trace".to_string(), "abc".to_string()),
            ("location".to_string(), "/api/items/1".to_string()),
        ]);

        let (begin, lease) = store.begin_lease("k", ttl, ttl).await.expect("begin");
        assert_eq!(begin, IdempotencyBegin::Started);
        store
            .complete_lease("k", lease.as_ref(), stored, ttl)
            .await
            .expect("complete");

        match store.begin("k", ttl).await.expect("replay") {
            IdempotencyBegin::Replay(cached) => assert_eq!(
                cached.headers,
                vec![
                    ("etag".to_string(), "\"v1\"".to_string()),
                    ("location".to_string(), "/api/items/1".to_string()),
                ],
                "only whitelisted headers are replayed, names are lower-cased"
            ),
            other => panic!("expected a replay, got {other:?}"),
        }
    }

    /// The reservation expires after the explicit lease, while a completed
    /// response replays for the retention TTL.
    #[tokio::test]
    async fn the_reservation_expires_after_the_lease_not_the_retention() {
        let store = InMemoryIdempotencyStore::new();
        let retention = Duration::from_secs(3_600);
        let (begin, _lease) = store
            .begin_lease("k", retention, Duration::from_millis(10))
            .await
            .expect("begin");
        assert_eq!(begin, IdempotencyBegin::Started);
        // The short lease has not expired yet...
        assert_eq!(
            store.begin("k", retention).await.expect("in flight"),
            IdempotencyBegin::InProgress
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        // ...after it, a retry may reclaim the key even though the retention is
        // still an hour away.
        assert_eq!(
            store.begin("k", retention).await.expect("reclaimed"),
            IdempotencyBegin::Started
        );

        // A completed response keeps the retention TTL, not the lease.
        store
            .complete("k", response(200, b"ok"), retention)
            .await
            .expect("complete");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(matches!(
            store.begin("k", retention).await.expect("still replayed"),
            IdempotencyBegin::Replay(_)
        ));
    }

    #[tokio::test]
    async fn eviction_prefers_completed_entries_over_live_reservations() {
        let store = InMemoryIdempotencyStore::with_max_keys(2);
        let long = Duration::from_secs(3_600);
        // Earliest-expiring entry is a live reservation; the completed entry
        // expires later. Eviction must still pick the completed one.
        store
            .begin("in-flight", Duration::from_secs(60))
            .await
            .unwrap();
        let (_, lease) = store.begin_lease("done", long, long).await.unwrap();
        store
            .complete_lease("done", lease.as_ref(), response(200, b"ok"), long)
            .await
            .unwrap();

        store.begin("new", long).await.expect("new reservation");
        assert_eq!(store.tracked_keys(), 2);
        // The live reservation survived...
        assert_eq!(
            store.begin("in-flight", long).await.expect("survived"),
            IdempotencyBegin::InProgress
        );
        // ...and the completed key was dropped, so a retry re-runs the handler.
        // (This insertion may evict the remaining reservation; that is fine, the
        // survival check above already ran.)
        assert_eq!(
            store.begin("done", long).await.expect("re-run"),
            IdempotencyBegin::Started
        );
    }

    #[tokio::test]
    async fn eviction_prefers_expired_entries_over_live_reservations() {
        let store = InMemoryIdempotencyStore::with_max_keys(2);
        let long = Duration::from_secs(3_600);
        store.begin("live", long).await.unwrap();
        store
            .begin("expiring", Duration::from_millis(10))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        store.begin("new", long).await.expect("new reservation");
        assert_eq!(
            store.begin("live", long).await.expect("live survived"),
            IdempotencyBegin::InProgress
        );
        assert_eq!(
            store.begin("new", long).await.expect("new survived"),
            IdempotencyBegin::InProgress
        );
    }

    #[test]
    fn path_prefixes_match_on_path_segment_boundaries() {
        assert!(path_prefix_matches("/api/orders", "/api/orders"));
        assert!(path_prefix_matches("/api/orders/123", "/api/orders"));
        assert!(!path_prefix_matches("/api/orders-archive", "/api/orders"));
        assert!(!path_prefix_matches("/api", "/api/orders"));
        // A trailing slash in the entry and a leading path slash are handled.
        assert!(path_prefix_matches("/api/orders/123", "/api/orders/"));
        assert!(path_prefix_matches("/api/orders", "/api/orders/"));
        // `/` and an empty entry cover every path.
        assert!(path_prefix_matches("/anything", "/"));
        assert!(path_prefix_matches("/anything", ""));
    }

    #[test]
    fn the_route_allow_list_defaults_to_everything_and_skip_wins() {
        let store: std::sync::Arc<dyn IIdempotencyStore> =
            std::sync::Arc::new(InMemoryIdempotencyStore::new());
        let ttl = Duration::from_secs(30);

        let everything = IdempotencyMiddleware::new(std::sync::Arc::clone(&store), ttl);
        assert!(everything.applies_to("/api/anything"));

        let scoped = IdempotencyMiddleware::new(std::sync::Arc::clone(&store), ttl).with_paths(
            vec!["/api/orders".to_string()],
            vec!["/api/orders/internal".to_string()],
        );
        assert!(scoped.applies_to("/api/orders"));
        assert!(scoped.applies_to("/api/orders/123"));
        // Path-segment boundary, not a string prefix.
        assert!(!scoped.applies_to("/api/orders-archive"));
        assert!(!scoped.applies_to("/api/users"));
        // SkipPaths wins over Paths.
        assert!(!scoped.applies_to("/api/orders/internal"));
        assert!(!scoped.applies_to("/api/orders/internal/job"));
    }

    #[test]
    fn request_scope_isolates_tenants_and_uses_the_client_info_address() {
        let ip = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
            203, 0, 113, 9,
        )));

        // The same subject in two tenants must not share a response cache.
        let tenant_a = scope_from_parts(
            Some("u-1"),
            Some("tenant-a"),
            ip,
            "post",
            "/api/orders",
            None,
        );
        let tenant_b = scope_from_parts(
            Some("u-1"),
            Some("tenant-b"),
            ip,
            "post",
            "/api/orders",
            None,
        );
        assert_ne!(tenant_a, tenant_b);
        assert_eq!(
            tenant_a.as_deref(),
            Some("tenant:tenant-a:user:u-1:POST:/api/orders")
        );

        // Same principal + method + path is stable (that is what replays).
        assert_eq!(
            tenant_a,
            scope_from_parts(
                Some("u-1"),
                Some("tenant-a"),
                ip,
                "POST",
                "/api/orders",
                None
            )
        );

        // A subject without a tenant claim stays user-scoped.
        assert_eq!(
            scope_from_parts(Some("u-1"), None, ip, "POST", "/api/x", None).as_deref(),
            Some("user:u-1:POST:/api/x")
        );
        // An empty tenant claim is not a tenant.
        assert_eq!(
            scope_from_parts(Some("u-1"), Some(""), ip, "POST", "/api/x", None).as_deref(),
            Some("user:u-1:POST:/api/x")
        );

        // Anonymous traffic is scoped by the resolved client address, never by
        // a missing/forged header. No address means no idempotency.
        assert_eq!(
            scope_from_parts(None, None, ip, "POST", "/api/x", None).as_deref(),
            Some("ip:203.0.113.9:POST:/api/x")
        );
        assert_eq!(
            scope_from_parts(None, None, None, "POST", "/api/x", None),
            None
        );

        // The query string is part of the scope: `?mode=a` and `?mode=b` are
        // different operations and must not replay each other.
        assert_eq!(
            scope_from_parts(Some("u-1"), Some("t"), ip, "POST", "/api/x", Some("mode=a"))
                .as_deref(),
            Some("tenant:t:user:u-1:POST:/api/x?mode=a")
        );
        assert_ne!(
            scope_from_parts(Some("u-1"), Some("t"), ip, "POST", "/api/x", Some("mode=a")),
            scope_from_parts(Some("u-1"), Some("t"), ip, "POST", "/api/x", Some("mode=b"))
        );
    }

    /// The scope encoding is injective: separators smuggled into a subject,
    /// tenant, path or query value cannot make two different requests share one
    /// idempotency key (a replay across principals/paths).
    #[test]
    fn the_scope_encoding_is_injective() {
        let ip = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
            203, 0, 113, 9,
        )));

        // A path containing the method delimiter must not alias a subject that
        // contains it: previously both produced `user:u:POST:/a:POST:/b`.
        let path_with_separators =
            scope_from_parts(Some("u"), None, ip, "POST", "/a:POST:/b", None).expect("scope A");
        let subject_with_separators =
            scope_from_parts(Some("u:POST:/a"), None, ip, "POST", "/b", None).expect("scope B");
        assert_ne!(path_with_separators, subject_with_separators);
        assert_eq!(
            path_with_separators,
            "user:u:POST:/a%3APOST%3A/b".to_string()
        );

        // A `?` in the path must not alias the path/query separator.
        assert_ne!(
            scope_from_parts(Some("u"), None, ip, "POST", "/x", Some("mode=a")),
            scope_from_parts(Some("u"), None, ip, "POST", "/x?mode=a", None)
        );

        // Tenants and subjects are escaped as well, so a tenant named like a
        // user segment cannot collide with another tenant's key space.
        assert_ne!(
            scope_from_parts(Some("u"), Some("t:user:u2"), ip, "POST", "/x", None),
            scope_from_parts(Some("u2"), Some("t"), ip, "POST", "/x", None)
        );

        // Ordinary values keep the legacy, human-readable form.
        assert_eq!(
            scope_from_parts(Some("u-1"), Some("t"), ip, "POST", "/api/orders", None).as_deref(),
            Some("tenant:t:user:u-1:POST:/api/orders")
        );
    }

    /// Escaping query keys/values keeps `{"a": "b&c=d"}` distinct from the two
    /// pairs `{"a": "b", "c": "d"}` — the canonical query is injective.
    #[test]
    fn the_canonical_query_is_injective() {
        let single_value = canonical_query_from_pairs(vec![("a".to_string(), "b&c=d".to_string())])
            .expect("query");
        let two_pairs = canonical_query_from_pairs(vec![
            ("a".to_string(), "b".to_string()),
            ("c".to_string(), "d".to_string()),
        ])
        .expect("query");
        assert_ne!(single_value, two_pairs);
        assert_eq!(single_value, "a=b%26c%3Dd");
        assert_eq!(two_pairs, "a=b&c=d");

        // The key is stable when a client reorders parameters.
        assert_eq!(
            canonical_query_from_pairs(vec![
                ("z".to_string(), "2".to_string()),
                ("a".to_string(), "1".to_string()),
            ]),
            canonical_query_from_pairs(vec![
                ("a".to_string(), "1".to_string()),
                ("z".to_string(), "2".to_string()),
            ])
        );
        assert_eq!(canonical_query_from_pairs(Vec::new()), None);
    }

    #[tokio::test]
    async fn the_store_is_bounded() {
        let store = InMemoryIdempotencyStore::with_max_keys(2);
        let ttl = Duration::from_secs(30);
        for key in ["a", "b", "c"] {
            store.begin(key, ttl).await.expect("begin");
        }
        assert_eq!(store.tracked_keys(), 2);
    }
}
