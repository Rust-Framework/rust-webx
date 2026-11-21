//! Exact response cache and key derivation.
//!
//! Only deterministic non-stream requests (`temperature == 0`) are cacheable.
//! The key includes the tenant, so a cache entry never crosses tenants.
//! Single-flight is deliberately not part of this module yet; callers can add
//! it in front of the executor without changing the key contract.

use crate::ir::CanonicalRequest;
use crate::ir::CanonicalResponse;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Response cache port.
#[async_trait]
pub trait IGatewayCache: Send + Sync {
    /// Look up a cached response; expired entries are misses.
    async fn get(&self, key: &str) -> Option<CanonicalResponse>;

    /// Store a response for `ttl`.
    async fn put(&self, key: String, response: CanonicalResponse, ttl: Duration);
}

/// Cache key for a request, or `None` when it is not cacheable.
///
/// Cacheable = non-stream + `temperature == Some(0.0)`. The tenant is hashed
/// into the key; model and all generation parameters are part of the request
/// serialization.
pub fn request_cache_key(tenant_id: &str, request: &CanonicalRequest) -> Option<String> {
    if request.stream || request.sampling.temperature != Some(0.0) {
        return None;
    }
    let serialized = serde_json::to_vec(request).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(tenant_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(serialized);
    let digest = hasher.finalize();
    let mut key = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(key, "{byte:02x}");
    }
    Some(key)
}

#[derive(Clone)]
struct Entry {
    response: CanonicalResponse,
    expires_at: Instant,
}

/// Process-local exact cache for tests/small deployments.
pub struct MemoryGatewayCache {
    entries: Mutex<HashMap<String, Entry>>,
    max_entries: usize,
}

impl MemoryGatewayCache {
    /// Cache at most `max_entries` responses.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_entries: max_entries.max(1),
        }
    }

    /// Number of stored (possibly expired) entries.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for MemoryGatewayCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[async_trait]
impl IGatewayCache for MemoryGatewayCache {
    async fn get(&self, key: &str) -> Option<CanonicalResponse> {
        let mut entries = self.lock();
        match entries.get(key) {
            Some(entry) if entry.expires_at > Instant::now() => Some(entry.response.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    async fn put(&self, key: String, response: CanonicalResponse, ttl: Duration) {
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.expires_at > Instant::now());
        if entries.len() >= self.max_entries {
            if let Some(victim) = entries.keys().next().cloned() {
                entries.remove(&victim);
            }
        }
        entries.insert(
            key,
            Entry {
                response,
                expires_at: Instant::now() + ttl,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Message, Role, SamplingParams};

    fn request() -> CanonicalRequest {
        CanonicalRequest::new("fast")
            .with_messages(vec![Message::text(Role::User, "hi")])
            .with_sampling(SamplingParams {
                temperature: Some(0.0),
                ..SamplingParams::default()
            })
    }

    fn response(id: &str) -> CanonicalResponse {
        CanonicalResponse {
            id: id.into(),
            model: "fast".into(),
            message: Message::text(Role::Assistant, "ok"),
            usage: crate::usage::Usage::ZERO,
            finish_reason: crate::ir::FinishReason::Stop,
        }
    }

    #[test]
    fn key_requires_zero_temperature_and_non_stream() {
        assert!(request_cache_key("tenant-1", &request()).is_some());
        let mut warm = request();
        warm.sampling.temperature = Some(0.7);
        assert!(request_cache_key("tenant-1", &warm).is_none());
        let streamed = request().stream();
        assert!(request_cache_key("tenant-1", &streamed).is_none());
    }

    #[test]
    fn key_is_tenant_and_payload_specific() {
        let first = request_cache_key("tenant-1", &request()).expect("cacheable");
        let second = request_cache_key("tenant-2", &request()).expect("cacheable");
        assert_ne!(first, second);

        let mut changed = request();
        changed.messages[0].content = vec![crate::ir::ContentPart::Text("other".into())];
        let third = request_cache_key("tenant-1", &changed).expect("cacheable");
        assert_ne!(first, third);
    }

    #[tokio::test]
    async fn memory_cache_hits_and_expires() {
        let cache = MemoryGatewayCache::new(2);
        cache
            .put("a".into(), response("a"), Duration::from_millis(100))
            .await;
        assert_eq!(cache.get("a").await.expect("hit").id, "a");
        assert_eq!(cache.len(), 1);

        cache
            .put("b".into(), response("b"), Duration::from_millis(1))
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(cache.get("b").await.is_none());
        assert_eq!(cache.len(), 1);
    }
}
