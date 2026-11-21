//! Short-TTL negative cache in front of [`ISessionStore`].
//!
//! Every authenticated request asks the store whether its `jti` was revoked.
//! With a database-backed store that is one round-trip per request even though
//! the answer is "not revoked" almost always. [`CachedSessionStore`] caches
//! only that negative answer for a few seconds; a local `revoke` invalidates
//! the entry immediately.
//!
//! Trade-off (documented, not hidden): a revocation performed by **another**
//! instance becomes visible after at most `ttl`. Keep the TTL small (1–5s) for
//! a multi-instance deployment, or broadcast invalidations from the control
//! plane.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use webx_core::error::Result;
use webx_core::security::ISessionStore;

/// Caches "not revoked" answers for `ttl`.
pub struct CachedSessionStore {
    inner: Arc<dyn ISessionStore>,
    ttl: Duration,
    max_entries: usize,
    /// `jti` → instant until which "not revoked" may be served from cache.
    not_revoked_until: Mutex<HashMap<String, Instant>>,
}

impl CachedSessionStore {
    /// Wrap `inner`; `ttl = Duration::ZERO` disables the cache.
    pub fn new(inner: Arc<dyn ISessionStore>, ttl: Duration) -> Self {
        Self {
            inner,
            // A multi-instance revocation delay must stay short; clamping also
            // makes `Instant::now() + ttl` overflow-proof.
            ttl: ttl.min(Duration::from_secs(300)),
            max_entries: 100_000,
            not_revoked_until: Mutex::new(HashMap::new()),
        }
    }

    /// Cap the number of cached `jti`s (default 100,000).
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    /// The wrapped store.
    pub fn inner(&self) -> &Arc<dyn ISessionStore> {
        &self.inner
    }

    /// Number of cached entries (diagnostics/tests).
    pub fn cached_entries(&self) -> usize {
        self.not_revoked_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn is_cached_not_revoked(&self, jti: &str) -> bool {
        if self.ttl.is_zero() {
            return false;
        }
        let now = Instant::now();
        let cache = self
            .not_revoked_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.get(jti).is_some_and(|until| *until > now)
    }

    fn insert(&self, jti: &str) {
        if self.ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let mut cache = self
            .not_revoked_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= self.max_entries {
            cache.retain(|_, until| *until > now);
            if cache.len() >= self.max_entries {
                cache.clear();
            }
        }
        cache.insert(jti.to_string(), now + self.ttl);
    }

    fn invalidate(&self, jti: &str) {
        self.not_revoked_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(jti);
    }
}

#[async_trait::async_trait]
impl ISessionStore for CachedSessionStore {
    async fn revoke(&self, jti: &str) -> Result<()> {
        self.inner.revoke(jti).await?;
        // The local caller expects the next request to see the revocation.
        self.invalidate(jti);
        Ok(())
    }

    async fn revoke_until(&self, jti: &str, expires_at_unix_secs: u64) -> Result<()> {
        self.inner.revoke_until(jti, expires_at_unix_secs).await?;
        self.invalidate(jti);
        Ok(())
    }

    async fn is_revoked(&self, jti: &str) -> Result<bool> {
        if self.is_cached_not_revoked(jti) {
            return Ok(false);
        }
        let revoked = self.inner.is_revoked(jti).await?;
        if revoked {
            // A revocation is authoritative; never serve a stale "not revoked"
            // for it.
            self.invalidate(jti);
        } else {
            self.insert(jti);
        }
        Ok(revoked)
    }

    async fn revoke_all_for_subject(&self, subject: &str) -> Result<()> {
        let result = self.inner.revoke_all_for_subject(subject).await;
        if result.is_ok() {
            self.not_revoked_until
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct CountingStore {
        calls: AtomicUsize,
        revoked: Mutex<HashSet<String>>,
    }

    #[async_trait::async_trait]
    impl ISessionStore for CountingStore {
        async fn revoke(&self, jti: &str) -> Result<()> {
            self.revoked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(jti.to_string());
            Ok(())
        }

        async fn is_revoked(&self, jti: &str) -> Result<bool> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .revoked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(jti))
        }

        async fn revoke_all_for_subject(&self, _subject: &str) -> Result<()> {
            self.revoked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
            Ok(())
        }
    }

    #[tokio::test]
    async fn not_revoked_answer_is_cached_within_ttl() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner.clone(), Duration::from_secs(60));

        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1, "cache hit");
        assert_eq!(store.cached_entries(), 1);
    }

    #[tokio::test]
    async fn local_revoke_invalidates_immediately() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner.clone(), Duration::from_secs(60));

        assert!(!store.is_revoked("jti-a").await.unwrap());
        store.revoke("jti-a").await.unwrap();
        assert!(store.is_revoked("jti-a").await.unwrap());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn revoked_answer_is_not_cached() {
        let inner = Arc::new(CountingStore::default());
        inner.revoke("jti-a").await.unwrap();
        let store = CachedSessionStore::new(inner.clone(), Duration::from_secs(60));

        assert!(store.is_revoked("jti-a").await.unwrap());
        assert!(store.is_revoked("jti-a").await.unwrap());
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            2,
            "a revoked token must keep asking the store"
        );
    }

    #[tokio::test]
    async fn cache_expires_after_ttl() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner.clone(), Duration::from_millis(30));

        assert!(!store.is_revoked("jti-a").await.unwrap());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn zero_ttl_disables_the_cache() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner.clone(), Duration::ZERO);
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
        assert_eq!(store.cached_entries(), 0);
    }

    #[test]
    fn oversized_ttl_is_clamped() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner, Duration::from_secs(86_400));
        assert_eq!(store.ttl, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn revoke_all_clears_the_cache() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner.clone(), Duration::from_secs(60));
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert!(!store.is_revoked("jti-b").await.unwrap());
        assert_eq!(store.cached_entries(), 2);

        store.revoke_all_for_subject("user-1").await.unwrap();
        assert_eq!(store.cached_entries(), 0);
    }
}
