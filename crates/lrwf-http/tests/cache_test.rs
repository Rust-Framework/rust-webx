//! Tests for MemoryCache implementing IDistributedCache.

use lrwf_core::cache::{
    DistributedCacheEntryOptions, DistributedCacheExtensions, IDistributedCache,
};
use lrwf_http::memory_cache::MemoryCache;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TestUser {
    id: u32,
    name: String,
}

#[tokio::test]
async fn set_and_get() {
    let c = MemoryCache::new();
    c.set("k", b"hello".to_vec(), None).await.unwrap();
    assert_eq!(c.get("k").await.unwrap(), Some(b"hello".to_vec()));
}

#[tokio::test]
async fn get_missing_returns_none() {
    let c = MemoryCache::new();
    assert_eq!(c.get("x").await.unwrap(), None);
}

#[tokio::test]
async fn exists_and_remove() {
    let c = MemoryCache::new();
    c.set("k", b"v".to_vec(), None).await.unwrap();
    assert!(c.exists("k").await.unwrap());
    c.remove("k").await.unwrap();
    assert!(!c.exists("k").await.unwrap());
}

#[tokio::test]
async fn clear_all() {
    let c = MemoryCache::new();
    c.set("a", b"1".to_vec(), None).await.unwrap();
    c.set("b", b"2".to_vec(), None).await.unwrap();
    c.clear().await.unwrap();
    assert_eq!(c.count().await, 0);
}

#[tokio::test]
async fn typed_get_set() {
    let c = MemoryCache::new();
    let u = TestUser {
        id: 1,
        name: "Alice".into(),
    };
    let opts = DistributedCacheEntryOptions::new();
    c.set_string("u:1", &u, &opts).await.unwrap();
    assert_eq!(c.get_string::<TestUser>("u:1").await.unwrap(), Some(u));
}

#[tokio::test]
async fn get_or_create() {
    let c = MemoryCache::new();
    let opts = DistributedCacheEntryOptions::new()
        .set_absolute_expiration_relative_to_now(Duration::from_secs(60));
    let u = c
        .get_or_create(
            "u:2",
            || async {
                TestUser {
                    id: 2,
                    name: "Bob".into(),
                }
            },
            &opts,
        )
        .await
        .unwrap();
    assert_eq!(u.name, "Bob");
    assert!(c.exists("u:2").await.unwrap());
}

#[tokio::test]
async fn sliding_extends_on_read() {
    let c = MemoryCache::new();
    let opts =
        DistributedCacheEntryOptions::new().set_sliding_expiration(Duration::from_secs(3600));
    c.set("s", b"v".to_vec(), Some(&opts)).await.unwrap();
    let _ = c.get("s").await.unwrap();
    assert!(c.exists("s").await.unwrap());
}

#[tokio::test]
async fn absolute_expiration_evicts() {
    let c = MemoryCache::new();
    let opts = DistributedCacheEntryOptions::new()
        .set_absolute_expiration_relative_to_now(Duration::from_millis(1));
    c.set("t", b"v".to_vec(), Some(&opts)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(c.get("t").await.unwrap(), None);
}

#[tokio::test]
async fn max_entries_enforced() {
    let c = MemoryCache::new().with_max_entries(3);
    for i in 0..5 {
        c.set(&format!("k{}", i), vec![i as u8], None)
            .await
            .unwrap();
    }
    assert!(c.count().await <= 3);
}

#[tokio::test]
async fn compact_removes_expired() {
    let c = MemoryCache::new();
    let opts = DistributedCacheEntryOptions::new()
        .set_absolute_expiration_relative_to_now(Duration::from_millis(1));
    c.set("exp", b"v".to_vec(), Some(&opts)).await.unwrap();
    c.set("keep", b"v".to_vec(), None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    c.compact().await;
    assert!(!c.exists("exp").await.unwrap());
    assert!(c.exists("keep").await.unwrap());
}

#[tokio::test]
async fn size_limit_rejected() {
    let c = MemoryCache::new();
    let opts = DistributedCacheEntryOptions::new().set_size_limit(5);
    assert!(c
        .set("k", b"too_large".to_vec(), Some(&opts))
        .await
        .is_err());
}

#[tokio::test]
async fn builder_pattern() {
    let opts = DistributedCacheEntryOptions::new()
        .set_absolute_expiration_relative_to_now(Duration::from_secs(300))
        .set_sliding_expiration(Duration::from_secs(60))
        .set_size_limit(1024);
    assert!(opts.absolute_expiration_relative_to_now.is_some());
    assert!(opts.sliding_expiration.is_some());
    assert_eq!(opts.size_limit, 1024);
}

#[tokio::test]
async fn count_after_operations() {
    let c = MemoryCache::new();
    assert_eq!(c.count().await, 0);
    c.set("a", b"1".to_vec(), None).await.unwrap();
    c.set("b", b"2".to_vec(), None).await.unwrap();
    assert_eq!(c.count().await, 2);
    c.remove("a").await.unwrap();
    assert_eq!(c.count().await, 1);
}
