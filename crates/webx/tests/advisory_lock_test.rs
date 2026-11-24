//! Advisory lock: host registration, in-memory lease semantics, no default lock.
//!
//! A process-local default would silently stop coordinating across instances, so
//! the host registers nothing unless the application asks for it.

use std::sync::Arc;
use std::time::Duration;

use webx::*;

#[tokio::test]
async fn a_host_registers_the_advisory_lock_port() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_advisory_lock(Arc::new(InMemoryAdvisoryLock::new()))
        .build();

    let lock = host
        .provider()
        .get::<dyn IAdvisoryLock>()
        .expect("the advisory lock port is registered");

    let lease = lock
        .try_acquire("quota:tenant-1", Duration::from_secs(5))
        .await
        .expect("acquire")
        .expect("free key");
    assert!(lock
        .try_acquire("quota:tenant-1", Duration::from_secs(5))
        .await
        .expect("contention")
        .is_none());
    assert!(lock.release(&lease).await.expect("release"));
}

#[tokio::test]
async fn a_held_key_can_be_awaited_with_a_bounded_timeout() {
    let lock = InMemoryAdvisoryLock::new();
    let held = lock
        .try_acquire("quota:tenant-1", Duration::from_secs(30))
        .await
        .expect("acquire")
        .expect("free key");

    // The framework-owned waiting policy: one attempt plus bounded backoff,
    // then `Ok(None)` at the deadline (never a spin loop in business code).
    let started = tokio::time::Instant::now();
    assert!(lock
        .acquire_with_timeout(
            "quota:tenant-1",
            Duration::from_secs(5),
            Duration::from_millis(40)
        )
        .await
        .expect("contention is not an error")
        .is_none());
    assert!(started.elapsed() >= Duration::from_millis(40));

    assert!(lock.release(&held).await.expect("release"));
    assert!(lock
        .acquire_with_timeout(
            "quota:tenant-1",
            Duration::from_secs(5),
            Duration::from_secs(1)
        )
        .await
        .expect("acquire")
        .is_some());
}

#[tokio::test]
async fn a_host_without_a_lock_never_silently_coordinates_locally() {
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    assert!(
        host.provider().get::<dyn IAdvisoryLock>().is_err(),
        "no process-local lock is installed by default"
    );
}

#[tokio::test]
async fn an_expired_lease_is_reclaimable_by_another_acquirer() {
    let lock = InMemoryAdvisoryLock::new();
    let stale = lock
        .try_acquire("job:daily", Duration::from_millis(20))
        .await
        .expect("acquire")
        .expect("free");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let current = lock
        .try_acquire("job:daily", Duration::from_secs(5))
        .await
        .expect("acquire")
        .expect("expired lease reclaimed");
    assert_ne!(stale.token, current.token);
    // The stale owner must not be able to release the new lease.
    assert!(!lock.release(&stale).await.expect("stale release"));
    assert!(lock.release(&current).await.expect("owner release"));
}
