//! API-key storage port and an in-memory implementation.
//!
//! The hot path is an indexed lookup by `key_id` plus one HMAC verification.
//! Revocation/expiry are checked on every request, so a revoked key stops
//! working immediately (no token cache to wait out).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Lifecycle state of an API key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApiKeyStatus {
    /// Usable until `expires_at_unix_secs`.
    #[default]
    Active,
    /// Permanently revoked.
    Revoked,
    /// Temporarily disabled by an operator.
    Suspended,
}

/// One stored API key. The secret is never stored in clear text.
#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    /// Indexed key id.
    pub key_id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// `HMAC-SHA256(pepper, secret)`.
    pub secret_hash: Vec<u8>,
    /// Which pepper produced `secret_hash` (supports rotation).
    pub pepper_id: String,
    /// Roles copied into claims.
    pub roles: Vec<String>,
    /// Permissions copied into claims.
    pub permissions: Vec<String>,
    /// Lifecycle state.
    pub status: ApiKeyStatus,
    /// Absolute expiry (Unix seconds); `None` = no expiry.
    pub expires_at_unix_secs: Option<u64>,
    /// Revocation time, for audit.
    pub revoked_at_unix_secs: Option<u64>,
}

impl ApiKeyRecord {
    /// Build an active record.
    pub fn new(
        key_id: impl Into<String>,
        tenant_id: impl Into<String>,
        secret_hash: impl Into<Vec<u8>>,
        pepper_id: impl Into<String>,
    ) -> Self {
        Self {
            key_id: key_id.into(),
            tenant_id: tenant_id.into(),
            secret_hash: secret_hash.into(),
            pepper_id: pepper_id.into(),
            roles: Vec::new(),
            permissions: Vec::new(),
            status: ApiKeyStatus::Active,
            expires_at_unix_secs: None,
            revoked_at_unix_secs: None,
        }
    }

    /// Add roles.
    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Add permissions.
    pub fn with_permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    /// Set an absolute expiry.
    pub fn with_expiry(mut self, expires_at_unix_secs: u64) -> Self {
        self.expires_at_unix_secs = Some(expires_at_unix_secs);
        self
    }

    /// Whether the key may authenticate at `now`.
    pub fn is_active_at(&self, now_unix_secs: u64) -> bool {
        self.status == ApiKeyStatus::Active
            && self.revoked_at_unix_secs.is_none()
            && self
                .expires_at_unix_secs
                .is_none_or(|expires| now_unix_secs < expires)
    }
}

/// Storage failure; details stay server-side.
#[derive(Debug, Error)]
pub enum ApiKeyStoreError {
    /// The backing store is unavailable.
    #[error("api key store unavailable: {0}")]
    Unavailable(String),
}

/// Lookup port for the API-key validator.
#[async_trait::async_trait]
pub trait IApiKeyStore: Send + Sync {
    /// Load a record by key id; `None` for unknown keys.
    async fn get(&self, key_id: &str) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError>;
}

/// In-memory store for tests, local development and small deployments.
#[derive(Debug, Default)]
pub struct MemoryApiKeyStore {
    records: Mutex<HashMap<String, ApiKeyRecord>>,
}

impl MemoryApiKeyStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a record.
    pub fn insert(&self, record: ApiKeyRecord) {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        records.insert(record.key_id.clone(), record);
    }

    /// Revoke a key; returns whether it existed.
    pub fn revoke(&self, key_id: &str) -> bool {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match records.get_mut(key_id) {
            Some(record) => {
                record.status = ApiKeyStatus::Revoked;
                record.revoked_at_unix_secs = Some(now_unix_secs());
                true
            }
            None => false,
        }
    }

    /// Remove a key entirely.
    pub fn remove(&self, key_id: &str) -> bool {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        records.remove(key_id).is_some()
    }

    /// Number of stored keys (admin/tests).
    pub fn len(&self) -> usize {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl IApiKeyStore for MemoryApiKeyStore {
    async fn get(&self, key_id: &str) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
        non_empty(key_id)?;
        let records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(records.get(key_id).cloned())
    }
}

fn non_empty(key_id: &str) -> Result<(), ApiKeyStoreError> {
    if key_id.is_empty() {
        Err(ApiKeyStoreError::Unavailable(
            "empty key id lookup".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// TTL cache in front of another [`IApiKeyStore`].
///
/// The hot path is an indexed lookup plus one HMAC; a database round-trip per
/// request would dominate it. This wrapper caches **positive** lookups for
/// `ttl`, so the key/tenant/roles snapshot survives a short cache window.
///
/// Revocation is the trade-off: a cached record outlives `revoke` until its TTL
/// (or an explicit [`CachedApiKeyStore::invalidate`]). An admin path that revokes
/// through the same instance should call `invalidate` so the key stops on the
/// next request; a process that revokes in another instance must therefore use a
/// short TTL (for example 5–30s) as its revocation window.
///
/// Negative lookups are **not** cached: a key created seconds ago must work on
/// the next request. `ttl == 0` disables caching entirely.
pub struct CachedApiKeyStore {
    inner: Arc<dyn IApiKeyStore>,
    ttl: Duration,
    entries: Mutex<HashMap<String, (Instant, ApiKeyRecord)>>,
}

impl CachedApiKeyStore {
    /// Wrap `inner` with a positive-result TTL.
    pub fn new(inner: Arc<dyn IApiKeyStore>, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The configured positive-cache lifetime.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Drop one cached record (call after revoking or rotating it).
    pub fn invalidate(&self, key_id: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key_id);
    }

    /// Drop every cached record (for example after a bulk import).
    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Number of live cached entries (diagnostics/tests).
    pub fn len(&self) -> usize {
        self.prune();
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Whether the cache holds no live entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn prune(&self) {
        if self.ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.retain(|_, (inserted, _)| now.duration_since(*inserted) < self.ttl);
    }
}

#[async_trait::async_trait]
impl IApiKeyStore for CachedApiKeyStore {
    async fn get(&self, key_id: &str) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
        non_empty(key_id)?;
        if !self.ttl.is_zero() {
            let cached = {
                let entries = self
                    .entries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                entries.get(key_id).and_then(|(inserted, record)| {
                    (inserted.elapsed() < self.ttl).then(|| record.clone())
                })
            };
            if let Some(record) = cached {
                return Ok(Some(record));
            }
        }

        let record = self.inner.get(key_id).await?;
        if let (Some(record), false) = (&record, self.ttl.is_zero()) {
            self.entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(key_id.to_string(), (Instant::now(), record.clone()));
        }
        Ok(record)
    }
}

/// Current Unix seconds.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_key::hash_secret;

    fn record() -> ApiKeyRecord {
        ApiKeyRecord::new(
            "key-1",
            "tenant-1",
            hash_secret(b"pepper", "0123456789abcdef").to_vec(),
            "v1",
        )
        .with_roles(["admin"])
    }

    #[tokio::test]
    async fn memory_store_inserts_and_revokes() {
        let store = MemoryApiKeyStore::new();
        store.insert(record());
        assert_eq!(store.len(), 1);
        assert!(store.get("key-1").await.expect("get").is_some());

        assert!(store.revoke("key-1"));
        let revoked = store.get("key-1").await.expect("get").expect("exists");
        assert_eq!(revoked.status, ApiKeyStatus::Revoked);
        assert!(!revoked.is_active_at(now_unix_secs()));
        assert!(!store.revoke("missing"));
    }

    #[test]
    fn expiry_and_revocation_disable_a_key() {
        let active = record().with_expiry(now_unix_secs() + 60);
        assert!(active.is_active_at(now_unix_secs()));

        let expired = record().with_expiry(1);
        assert!(!expired.is_active_at(now_unix_secs()));

        let mut revoked = record();
        revoked.status = ApiKeyStatus::Revoked;
        revoked.revoked_at_unix_secs = Some(now_unix_secs());
        assert!(!revoked.is_active_at(now_unix_secs()));
    }

    #[tokio::test]
    async fn empty_lookup_is_an_error() {
        let store = MemoryApiKeyStore::new();
        assert!(store.get("").await.is_err());
    }

    /// Store that counts lookups so cache behaviour is observable.
    struct CountingStore {
        inner: MemoryApiKeyStore,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl CountingStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: MemoryApiKeyStore::new(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl IApiKeyStore for CountingStore {
        async fn get(&self, key_id: &str) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.get(key_id).await
        }
    }

    #[tokio::test]
    async fn cached_store_serves_repeat_lookups_from_the_cache() {
        let inner = CountingStore::new();
        inner.inner.insert(record());
        let store = CachedApiKeyStore::new(inner.clone(), Duration::from_secs(60));

        let first = store.get("key-1").await.expect("first").expect("record");
        let second = store.get("key-1").await.expect("second").expect("record");
        assert_eq!(first.key_id, second.key_id);
        assert_eq!(
            inner.calls(),
            1,
            "the second lookup must not reach the store"
        );

        store.invalidate("key-1");
        let _ = store.get("key-1").await.expect("third");
        assert_eq!(inner.calls(), 2, "invalidate forces a fresh lookup");
    }

    #[tokio::test]
    async fn cached_entries_expire() {
        let inner = CountingStore::new();
        inner.inner.insert(record());
        let store = CachedApiKeyStore::new(inner.clone(), Duration::from_millis(10));

        let _ = store.get("key-1").await.expect("first");
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = store.get("key-1").await.expect("second");
        assert_eq!(inner.calls(), 2, "an expired entry is re-fetched");
    }

    #[tokio::test]
    async fn unknown_keys_are_not_cached() {
        let inner = CountingStore::new();
        let store = CachedApiKeyStore::new(inner.clone(), Duration::from_secs(60));

        assert!(store.get("missing").await.expect("first").is_none());
        assert!(store.get("missing").await.expect("second").is_none());
        assert_eq!(
            inner.calls(),
            2,
            "a key created after the first miss must work immediately"
        );
    }

    #[tokio::test]
    async fn zero_ttl_disables_the_cache() {
        let inner = CountingStore::new();
        inner.inner.insert(record());
        let store = CachedApiKeyStore::new(inner.clone(), Duration::ZERO);

        let _ = store.get("key-1").await.expect("first");
        let _ = store.get("key-1").await.expect("second");
        assert_eq!(inner.calls(), 2);
        assert!(store.is_empty());
    }
}
