//! 2.5 M1 — revocation cache wiring (`CachedSessionStore`).

mod test_utils;

use std::sync::Arc;
use std::time::Duration;

use webx_core::mode::AppMode;
use webx_core::security::{ISessionStore, InMemorySessionStore};
use webx_host::server::Host;
use webx_host::CachedSessionStore;

#[tokio::test]
async fn cached_store_is_registered_and_serves_revocations() {
    let inner = Arc::new(InMemorySessionStore::new());
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_session_store(Arc::new(CachedSessionStore::new(
            inner,
            Duration::from_secs(5),
        )))
        .try_build()
        .expect("host builds");

    let store = host
        .provider()
        .get::<dyn ISessionStore>()
        .expect("cached store is registered as dyn ISessionStore");

    assert!(!store.is_revoked("jti-a").await.expect("check"));
    store.revoke("jti-a").await.expect("revoke");
    assert!(
        store.is_revoked("jti-a").await.expect("check"),
        "a local revoke must be visible immediately through the cache"
    );
}
