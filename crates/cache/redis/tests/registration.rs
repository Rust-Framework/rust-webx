//! `add_redis` wires the cache and event transport without requiring a live
//! server (the connection is lazy).

use webx_core::cache::IDistributedCache;
use webx_core::locking::IAdvisoryLock;
use webx_core::mediator::IEventPublisher;
use webx_core::mode::AppMode;
use webx_host::server::Host;
use webx_redis::{RedisCache, RedisHostBuilderExt, RedisOptions, RedisServiceCollectionExt};

#[test]
fn add_redis_registers_cache_and_event_transport() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .add_redis(RedisOptions::new("redis://127.0.0.1:1"))
        .try_build()
        .expect("host builds without connecting to Redis");

    assert!(host.provider().get::<RedisCache>().is_ok());
    assert!(host.provider().get::<dyn IDistributedCache>().is_ok());
    assert!(host.provider().get::<dyn IEventPublisher>().is_ok());
}

#[test]
fn cache_only_does_not_install_an_event_publisher() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .add_redis(RedisOptions::new("redis://127.0.0.1:1").cache_only())
        .try_build()
        .expect("host builds");

    assert!(host.provider().get::<dyn IDistributedCache>().is_ok());
    assert!(host.provider().get::<dyn IEventPublisher>().is_err());
}

#[test]
fn service_collection_registration_is_usable_from_register() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .register(|services| {
            services.add_redis(RedisOptions::new("redis://127.0.0.1:1").without_cache())
        })
        .try_build()
        .expect("host builds");

    assert!(host.provider().get::<dyn IEventPublisher>().is_ok());
    assert!(host.provider().get::<dyn IDistributedCache>().is_err());
}

#[test]
fn add_redis_advisory_lock_registers_only_the_lock() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .add_redis_advisory_lock(RedisOptions::new("redis://127.0.0.1:1").with_prefix("app:"))
        .try_build()
        .expect("host builds without connecting");

    assert!(host.provider().get::<dyn IAdvisoryLock>().is_ok());
    assert!(host.provider().get::<dyn IDistributedCache>().is_err());
    assert!(host.provider().get::<dyn IEventPublisher>().is_err());
}
