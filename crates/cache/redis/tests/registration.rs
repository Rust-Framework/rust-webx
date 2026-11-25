//! `add_redis` wires the cache and event transport without requiring a live
//! server (the connection is lazy).

use std::sync::Arc;
use std::time::Duration;

use webx_core::cache::IDistributedCache;
use webx_core::locking::IAdvisoryLock;
use webx_core::mediator::IEventPublisher;
use webx_core::mode::AppMode;
use webx_core::realtime::{IOnlinePresence, IRealtimeEventStore};
use webx_host::server::Host;
use webx_redis::{
    RedisCache, RedisConnection, RedisEventConsumer, RedisHostBuilderExt, RedisOnlinePresence,
    RedisOptions, RedisRealtimeEventStore, RedisServiceCollectionExt,
};

fn lazy_connection() -> Arc<RedisConnection> {
    Arc::new(RedisConnection::new(
        "redis://127.0.0.1:1",
        Duration::from_secs(1),
        Duration::from_secs(1),
    ))
}

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
    // Reuse and operations entry points: the shared connection is a singleton
    // and the consumer is resolvable for dead-letter replay.
    assert!(host.provider().get::<RedisConnection>().is_ok());
    assert!(host.provider().get::<RedisEventConsumer>().is_ok());
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
    // A cache-only registration installs no event consumer either.
    assert!(host.provider().get::<RedisEventConsumer>().is_err());
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

#[test]
fn redis_realtime_ports_wire_through_the_host_use_entries() {
    let connection = lazy_connection();
    let store = Arc::new(RedisRealtimeEventStore::new(Arc::clone(&connection)));
    let presence = Arc::new(RedisOnlinePresence::new(Arc::clone(&connection)));

    let host = Host::builder()
        .mode(AppMode::Development)
        .use_realtime_event_store(store)
        .use_online_presence(presence)
        .try_build()
        .expect("host builds without connecting to Redis");

    assert!(host.provider().get::<dyn IRealtimeEventStore>().is_ok());
    assert!(host.provider().get::<dyn IOnlinePresence>().is_ok());
}

#[test]
fn redis_realtime_ports_can_be_registered_in_di_directly() {
    let connection = lazy_connection();
    let store: Arc<dyn IRealtimeEventStore> =
        Arc::new(RedisRealtimeEventStore::new(Arc::clone(&connection)));
    let presence: Arc<dyn IOnlinePresence> =
        Arc::new(RedisOnlinePresence::new(Arc::clone(&connection)));

    let host = Host::builder()
        .mode(AppMode::Development)
        .register(move |services| {
            services
                .instance::<dyn IRealtimeEventStore>(store)
                .instance::<dyn IOnlinePresence>(presence)
        })
        .try_build()
        .expect("host builds without connecting");

    assert!(host.provider().get::<dyn IRealtimeEventStore>().is_ok());
    assert!(host.provider().get::<dyn IOnlinePresence>().is_ok());
}
