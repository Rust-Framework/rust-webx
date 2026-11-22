//! Request-level idempotency: in-memory store + `Idempotency-Key` middleware.
//!
//! [`InMemoryIdempotencyStore`] is the single-process implementation of
//! [`IIdempotencyStore`] (bounded key count, TTL-swept on access).
//! [`IdempotencyMiddleware`] is a continuation-style middleware: it reserves the
//! key, runs the rest of the pipeline at most once, and stores or releases the
//! response.
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

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::idempotency::{
    IIdempotencyStore, IdempotencyBegin, IdempotentResponse, IDEMPOTENCY_KEY_HEADER,
    IDEMPOTENCY_REPLAYED_HEADER,
};
use webx_core::middleware::{IMiddleware, NextStage};

use crate::problem_response::write_problem;

struct Entry {
    expires_at: Instant,
    /// `None` while the request that reserved the key is still running.
    response: Option<IdempotentResponse>,
}

/// Process-local idempotency store.
///
/// Bounded by `max_keys` (the entry with the earliest expiry is evicted);
/// expired entries are pruned on every access, so no background task is needed.
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
}

fn prune(entries: &mut HashMap<String, Entry>, now: Instant) {
    entries.retain(|_, entry| entry.expires_at > now);
}

fn evict_one(entries: &mut HashMap<String, Entry>) {
    if let Some(key) = entries
        .iter()
        .min_by_key(|(_, entry)| entry.expires_at)
        .map(|(key, _)| key.clone())
    {
        entries.remove(&key);
    }
}

#[async_trait]
impl IIdempotencyStore for InMemoryIdempotencyStore {
    async fn begin(&self, key: &str, ttl: Duration) -> Result<IdempotencyBegin> {
        let now = Instant::now();
        let mut entries = self.lock();
        prune(&mut entries, now);
        if let Some(entry) = entries.get(key) {
            return Ok(match &entry.response {
                Some(response) => IdempotencyBegin::Replay(response.clone()),
                None => IdempotencyBegin::InProgress,
            });
        }
        while entries.len() >= self.max_keys {
            evict_one(&mut entries);
        }
        entries.insert(
            key.to_string(),
            Entry {
                expires_at: now + ttl,
                response: None,
            },
        );
        Ok(IdempotencyBegin::Started)
    }

    async fn complete(&self, key: &str, response: IdempotentResponse, ttl: Duration) -> Result<()> {
        let now = Instant::now();
        let mut entries = self.lock();
        prune(&mut entries, now);
        if let Some(entry) = entries.get_mut(key) {
            // A completed key keeps its response; a reservation that expired (or
            // was evicted) is not resurrected by a late completion.
            if entry.response.is_none() {
                entry.response = Some(response);
                entry.expires_at = now + ttl;
            }
        }
        Ok(())
    }

    async fn release(&self, key: &str) -> Result<()> {
        let now = Instant::now();
        let mut entries = self.lock();
        prune(&mut entries, now);
        let reserved = entries
            .get(key)
            .is_some_and(|entry| entry.response.is_none());
        if reserved {
            entries.remove(key);
        }
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
    max_body_bytes: usize,
}

impl IdempotencyMiddleware {
    /// Middleware over `store` with a per-key TTL (default body cap: 1 MiB).
    pub fn new(store: std::sync::Arc<dyn IIdempotencyStore>, ttl: Duration) -> Self {
        Self {
            store,
            ttl,
            max_body_bytes: 1024 * 1024,
        }
    }

    /// Change the largest response body that is cached (larger ones release the key).
    pub fn with_max_body_bytes(mut self, max_body_bytes: usize) -> Self {
        self.max_body_bytes = max_body_bytes.max(1);
        self
    }
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
        if !is_state_changing(ctx.request().method()) {
            return next.run(ctx).await;
        }
        let Some(scope) = request_scope(ctx) else {
            return next.run(ctx).await;
        };
        let key = format!("idem:{scope}:{presented}");

        match self.store.begin(&key, self.ttl).await? {
            IdempotencyBegin::Started => {
                let result = next.run(ctx).await;
                let status = ctx.response().status();
                let body = ctx.response().body_bytes();
                let stream = ctx
                    .response()
                    .header("content-type")
                    .is_some_and(|content_type| content_type.starts_with("text/event-stream"));
                let cacheable = result.is_ok()
                    && (200..300).contains(&status)
                    && !stream
                    && body.len() <= self.max_body_bytes;
                if cacheable {
                    let response = IdempotentResponse {
                        status,
                        content_type: ctx.response().header("content-type").map(str::to_string),
                        content_encoding: ctx
                            .response()
                            .header("content-encoding")
                            .map(str::to_string),
                        body,
                    };
                    if let Err(error) = self.store.complete(&key, response, self.ttl).await {
                        tracing::warn!(%error, "idempotency: storing the response failed");
                    }
                } else if let Err(error) = self.store.release(&key).await {
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
                response.set_header(IDEMPOTENCY_REPLAYED_HEADER, "true");
                response.write_bytes(cached.body).await
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

/// Principal + method + path, so two clients cannot collide on one key and a
/// retry to a different route is not replayed.
fn request_scope(ctx: &dyn IHttpContext) -> Option<String> {
    let subject = ctx
        .claims()
        .map(|claims| claims.subject().to_string())
        .filter(|subject| !subject.is_empty())
        .map(|subject| format!("user:{subject}"))
        .or_else(|| {
            ctx.request()
                .remote_addr()
                .map(|addr| format!("ip:{}", addr.ip()))
        })?;
    Some(format!(
        "{subject}:{}:{}",
        ctx.request().method().to_ascii_uppercase(),
        ctx.request().path()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &[u8]) -> IdempotentResponse {
        IdempotentResponse {
            status,
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: body.to_vec(),
        }
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
    async fn the_store_is_bounded() {
        let store = InMemoryIdempotencyStore::with_max_keys(2);
        let ttl = Duration::from_secs(30);
        for key in ["a", "b", "c"] {
            store.begin(key, ttl).await.expect("begin");
        }
        assert_eq!(store.tracked_keys(), 2);
    }
}
