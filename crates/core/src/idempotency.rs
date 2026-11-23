//! Request-level idempotency (`Idempotency-Key`).
//!
//! A client that retries a state-changing request (a payment, an order, an
//! invitation) sends the same `Idempotency-Key`. The framework reserves the key,
//! runs the handler at most once, stores the successful response, and replays it
//! for later retries. A concurrent duplicate gets `409` while the first request
//! is still in flight.
//!
//! The port is a three-step protocol so a durable store can be implemented
//! without changing the middleware:
//!
//! ```text
//! begin(key) ──► Started      run handler; complete(key, response) or release(key)
//!            ├─► InProgress   another request owns the key → 409 + Retry-After
//!            └─► Replay(rsp)  stored response → replay bytes, no handler
//! ```
//!
//! # Leases and fencing tokens
//!
//! A reservation can expire (or be evicted) while the handler that owns it is
//! still running; a retry then reclaims the key, and the first attempt's late
//! `complete`/`release` must not touch the new attempt. Stores that can tell
//! attempts apart implement the optional **lease** methods
//! ([`IIdempotencyStore::begin_lease`] and friends): `begin_lease` returns an
//! opaque [`IdempotencyLease`] token with `Started`, and `complete_lease` /
//! `release_lease` only take effect while that exact token still owns the key.
//! A stale attempt's late `complete`/`release` is a no-op, even while the new
//! lease is live.
//!
//! The lease protocol has **two** durations, both chosen by the caller:
//!
//! * `retention` — how long a *completed* response is replayed before the key
//!   may execute again (the middleware's `Idempotency.TtlSeconds`);
//! * `lease` — how long an *in-flight reservation* blocks concurrent retries
//!   before another attempt may reclaim the key (the middleware's
//!   `Idempotency.LeaseSeconds`, capped at `retention`).
//!
//! `begin_lease` receives both; `complete_lease` receives only `retention`
//! (it stores a completed response) and `release_lease` neither. The default
//! implementations delegate to the token-less `begin`/`complete`/`release`
//! with the retention period, so stores written before 2.8 keep working
//! unchanged — they simply keep the weaker "TTL must exceed the handler's worst
//! case" guarantee and ignore the shorter lease.
//!
//! # Replayable response headers
//!
//! [`IdempotentResponse::headers`] carries the response headers stored and
//! replayed together with the body. The middleware captures exactly
//! [`REPLAYABLE_RESPONSE_HEADERS`] in this order (Location, ETag, Last-Modified,
//! Cache-Control); other headers are transport/framework specific and are not
//! replayed. A store may only replay headers from that whitelist.
//!
//! # Constructing responses
//!
//! [`IdempotentResponse`] is `#[non_exhaustive]`: construct it with
//! [`IdempotentResponse::new`] and add replayable headers with
//! [`IdempotentResponse::with_headers`]. [`IdempotencyBegin`] and
//! [`IdempotencyLease`] are also `#[non_exhaustive]`; the variants and
//! [`IdempotencyLease::new`] remain constructible, but a `match` on the enum
//! needs a wildcard arm.
//!
//! # Deployment semantics
//!
//! * **Single process**: `webx_host::idempotency::InMemoryIdempotencyStore`
//!   (bounded, TTL-swept) is enough.
//! * **Multiple instances**: retries can land on another node, so a shared store
//!   (Redis `SET NX` + response blob, or a database table with a unique key) is
//!   required; the port is the only contract.
//! * **Containers**: keys carry a TTL, so a killed node's reservation expires and
//!   the client's next retry proceeds; nothing is stored on local disk.
//! * **Performance**: one store round trip before and one after the handler;
//!   responses are cached in memory/bytes, so bound the body size and key count.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::Result;

/// Header carrying the client-generated idempotency key.
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// Header set on a replayed response.
pub const IDEMPOTENCY_REPLAYED_HEADER: &str = "idempotency-replayed";

/// Response headers captured and replayed with a cached response.
///
/// Names are lower-case (HTTP header lookup is case-insensitive) and the array
/// order is the storage/replay order. The list is a security boundary: only
/// headers that describe the cached representation are replayed — never
/// `Set-Cookie`, `Authorization` or other credential-bearing headers.
pub const REPLAYABLE_RESPONSE_HEADERS: [&str; 4] =
    ["location", "etag", "last-modified", "cache-control"];

/// Default reservation lease (`Idempotency.LeaseSeconds`): 60 seconds.
///
/// The lease is how long an in-flight reservation blocks concurrent retries
/// before another attempt may reclaim the key. It is separate from the replay
/// retention period; a crashed handler becomes retryable after the lease, while
/// a stored response replays for the whole retention period.
pub const DEFAULT_LEASE: Duration = Duration::from_secs(60);

/// Stored response bytes for replay.
///
/// `#[non_exhaustive]`: construct it with [`Self::new`] and add replayable
/// headers with [`Self::with_headers`] (a struct literal is not available
/// outside this crate).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdempotentResponse {
    /// HTTP status of the original response.
    pub status: u16,
    /// `content-type` of the original response (not framework headers).
    pub content_type: Option<String>,
    /// `content-encoding` of the original response (compressed bytes are replayed as-is).
    pub content_encoding: Option<String>,
    /// Response headers captured from [`REPLAYABLE_RESPONSE_HEADERS`], in capture
    /// order. Empty when the original response carried none of them.
    pub headers: Vec<(String, String)>,
    /// Body bytes of the original response.
    pub body: Vec<u8>,
}

impl IdempotentResponse {
    /// A response with no replayable headers.
    pub fn new(
        status: u16,
        content_type: Option<String>,
        content_encoding: Option<String>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            content_type,
            content_encoding,
            headers: Vec::new(),
            body,
        }
    }

    /// Attach the replayable response headers (see [`REPLAYABLE_RESPONSE_HEADERS`]).
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = headers;
        self
    }
}

/// Fencing token of one reservation attempt.
///
/// Opaque to the middleware: the store generates it when it grants a
/// reservation and compares it again on `complete_lease`/`release_lease`, so an
/// attempt whose lease expired (and was reclaimed) cannot overwrite the new
/// owner's state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdempotencyLease {
    /// Store-generated opaque attempt token.
    pub token: String,
}

impl IdempotencyLease {
    /// Wrap a store-generated token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

/// Result of reserving an idempotency key.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdempotencyBegin {
    /// This request owns the key; run the handler then `complete` or `release`.
    Started,
    /// Another request is still running with this key.
    InProgress,
    /// A response is already stored; replay it without running the handler.
    Replay(IdempotentResponse),
}

/// Idempotency store for state-changing requests.
///
/// Keys are opaque; the middleware builds them from the principal, method, path
/// and the client header, so the store never needs to interpret them.
///
/// Implement [`Self::begin`], [`Self::complete`] and [`Self::release`]. To make
/// late writes from a reclaimed reservation impossible, also implement the
/// lease variants ([`Self::begin_lease`], [`Self::complete_lease`],
/// [`Self::release_lease`]) — the middleware always calls the lease variants,
/// which default to the token-less ones.
#[async_trait]
pub trait IIdempotencyStore: Send + Sync {
    /// Reserve `key` for at most `ttl` — the same duration bounds the stored
    /// response of a completed request.
    ///
    /// Legacy/compact entry point: it does not take a separate lease. The
    /// middleware uses [`Self::begin_lease`] instead; this method is what the
    /// default implementations fall back to, and what stores written before 2.8
    /// implement directly.
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin>;

    /// Store the response produced by the request that reserved `key`, replayable
    /// for `ttl`.
    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()>;

    /// Drop a reservation that produced no cacheable response (handler error or
    /// a non-2xx status), so a retry can proceed.
    async fn release(&self, key: &str) -> Result<()>;

    /// Reserve `key` and return the fencing token of this attempt.
    ///
    /// Called by the middleware in place of [`Self::begin`]. `retention` is how
    /// long a completed response must replay; `lease` is how long this in-flight
    /// reservation blocks concurrent retries before another attempt may reclaim
    /// it. The caller passes `lease <= retention` (the middleware caps the
    /// configured `Idempotency.LeaseSeconds` at the retention TTL).
    ///
    /// The default implementation keeps the pre-2.8 behavior: no token and no
    /// separate lease — it delegates to [`Self::begin`] with `retention`, so the
    /// reservation lasts the full retention period.
    async fn begin_lease(
        &self,
        key: &str,
        retention: Duration,
        lease: Duration,
    ) -> Result<(IdempotencyBegin, Option<IdempotencyLease>)> {
        let _ = lease;
        Ok((self.begin(key, retention).await?, None))
    }

    /// Store the response only if `lease` still owns `key`; it replays for
    /// `retention`.
    ///
    /// `lease` is the token returned by [`Self::begin_lease`] for this attempt;
    /// `None` means the store handed out no token (default/legacy path).
    async fn complete_lease(
        &self,
        key: &str,
        lease: Option<&IdempotencyLease>,
        response: IdempotentResponse,
        retention: Duration,
    ) -> Result<()> {
        let _ = lease;
        self.complete(key, response, retention).await
    }

    /// Release the reservation only if `lease` still owns `key`.
    async fn release_lease(&self, key: &str, lease: Option<&IdempotencyLease>) -> Result<()> {
        let _ = lease;
        self.release(key).await
    }
}
