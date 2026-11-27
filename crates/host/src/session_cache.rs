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
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Bumped by every local revocation. An in-flight `is_revoked` that read a
    /// "not revoked" answer before the bump must not cache it afterwards.
    revision: AtomicU64,
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
            revision: AtomicU64::new(0),
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

    /// Record a local revocation: bump the revision first, then drop the entry,
    /// so a concurrent reader cannot re-insert a stale negative answer.
    fn record_local_revocation(&self, jti: Option<&str>) {
        self.revision.fetch_add(1, Ordering::SeqCst);
        match jti {
            Some(jti) => self.invalidate(jti),
            None => {
                self.not_revoked_until
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clear();
            }
        }
    }
}

#[async_trait::async_trait]
impl ISessionStore for CachedSessionStore {
    async fn revoke(&self, jti: &str) -> Result<()> {
        self.inner.revoke(jti).await?;
        // The local caller expects the next request to see the revocation.
        self.record_local_revocation(Some(jti));
        Ok(())
    }

    async fn revoke_until(&self, jti: &str, expires_at_unix_secs: u64) -> Result<()> {
        self.inner.revoke_until(jti, expires_at_unix_secs).await?;
        self.record_local_revocation(Some(jti));
        Ok(())
    }

    async fn is_revoked(&self, jti: &str) -> Result<bool> {
        if self.is_cached_not_revoked(jti) {
            return Ok(false);
        }
        let revision = self.revision.load(Ordering::SeqCst);
        let revoked = self.inner.is_revoked(jti).await?;
        if revoked {
            // A revocation is authoritative; never serve a stale "not revoked"
            // for it.
            self.invalidate(jti);
        } else if self.revision.load(Ordering::SeqCst) == revision {
            // Only cache the negative answer if no local revocation happened
            // while the store was being read: otherwise the entry would mask
            // a revocation for one TTL.
            self.insert(jti);
        }
        Ok(revoked)
    }

    async fn revoke_all_for_subject(&self, subject: &str) -> Result<()> {
        let result = self.inner.revoke_all_for_subject(subject).await;
        if result.is_ok() {
            self.record_local_revocation(None);
        }
        result
    }

    fn supports_refresh_family_revocation(&self) -> bool {
        // The cached layer must not turn a capable inner store into a
        // "capability missing" one: the declaration is the inner store's.
        self.inner.supports_refresh_family_revocation()
    }

    async fn revoke_by_refresh_family(&self, refresh_family_id: &str) -> Result<u64> {
        // Forward the optional family→session capability; without this override
        // wrapping a capable store in the cache would silently degrade every
        // paired revocation to the trait's 501 default.
        let result = self.inner.revoke_by_refresh_family(refresh_family_id).await;
        if result.is_ok() {
            // Which jti values were revoked is the inner store's knowledge, so
            // drop the whole negative cache (same reasoning as sign-out-all).
            self.record_local_revocation(None);
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

        async fn revoke_by_refresh_family(&self, _refresh_family_id: &str) -> Result<u64> {
            self.revoked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
            Ok(1)
        }

        fn supports_refresh_family_revocation(&self) -> bool {
            true
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

    #[tokio::test]
    async fn family_revocation_is_forwarded_and_clears_the_cache() {
        let inner = Arc::new(CountingStore::default());
        let store = CachedSessionStore::new(inner.clone(), Duration::from_secs(60));
        assert!(!store.is_revoked("jti-a").await.unwrap());
        assert_eq!(store.cached_entries(), 1);

        // Forwarding the optional capability must not degrade to the 501 default.
        let revoked = store.revoke_by_refresh_family("family-1").await.unwrap();
        assert_eq!(revoked, 1);
        assert_eq!(
            store.cached_entries(),
            0,
            "an unknown set of jti values was revoked: drop the negative cache"
        );
    }

    #[test]
    fn refresh_family_capability_is_forwarded() {
        let capable = Arc::new(CountingStore::default());
        assert!(
            CachedSessionStore::new(capable, Duration::from_secs(60))
                .supports_refresh_family_revocation(),
            "wrapping a capable store must not hide its capability"
        );

        let unsupported = Arc::new(webx_core::security::InMemorySessionStore::new());
        assert!(
            !CachedSessionStore::new(unsupported, Duration::ZERO)
                .supports_refresh_family_revocation(),
            "a store without a family index stays unsupported through the wrapper"
        );
    }

    /// An `is_revoked` read that overlaps a local `revoke` must not cache its
    /// (now stale) "not revoked" answer: that would mask the revocation for one
    /// TTL, contradicting "a local revoke invalidates immediately".
    #[tokio::test]
    async fn concurrent_revoke_does_not_leave_a_negative_entry() {
        use tokio::sync::Notify;

        struct SlowNegativeStore {
            started: Notify,
            release: Notify,
        }

        #[async_trait::async_trait]
        impl ISessionStore for SlowNegativeStore {
            async fn revoke(&self, _jti: &str) -> Result<()> {
                Ok(())
            }

            async fn is_revoked(&self, _jti: &str) -> Result<bool> {
                self.started.notify_one();
                self.release.notified().await;
                Ok(false)
            }
        }

        let inner = Arc::new(SlowNegativeStore {
            started: Notify::new(),
            release: Notify::new(),
        });
        let store = Arc::new(CachedSessionStore::new(
            Arc::clone(&inner) as Arc<dyn ISessionStore>,
            Duration::from_secs(60),
        ));

        let check = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.is_revoked("jti-a").await })
        };
        inner.started.notified().await;
        // Revoke while the store read is still in flight.
        store.revoke("jti-a").await.expect("revoke");
        inner.release.notify_one();

        assert!(!check.await.expect("join").expect("check"));
        assert_eq!(
            store.cached_entries(),
            0,
            "a late negative entry must not mask a completed local revoke"
        );
    }
}
