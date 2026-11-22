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

/// Stored response bytes for replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotentResponse {
    /// HTTP status of the original response.
    pub status: u16,
    /// `content-type` of the original response (not framework headers).
    pub content_type: Option<String>,
    /// `content-encoding` of the original response (compressed bytes are replayed as-is).
    pub content_encoding: Option<String>,
    /// Body bytes of the original response.
    pub body: Vec<u8>,
}

/// Result of reserving an idempotency key.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[async_trait]
pub trait IIdempotencyStore: Send + Sync {
    /// Reserve `key` (creating it when absent) for at most `ttl`.
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin>;

    /// Store the response produced by the request that reserved `key`.
    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()>;

    /// Drop a reservation that produced no cacheable response (handler error or
    /// a non-2xx status), so a retry can proceed.
    async fn release(&self, key: &str) -> Result<()>;
}
