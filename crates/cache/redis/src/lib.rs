//! Redis integration for rust-webx: distributed cache, event transport and
//! shared real-time state.
//!
//! ASP.NET Core style: add the crate, call [`add_redis`](RedisHostBuilderExt::add_redis),
//! and both `IDistributedCache` and `IEventPublisher` are wired against the same
//! multiplexed connection. A background consumer reads competing events from a
//! consumer group on the stream and subscribes to the broadcast Pub/Sub channel,
//! then dispatches both through the application's `#[handler]` event handlers.
//!
//! The crate also ships the Redis implementations of the two real-time ports:
//! [`RedisRealtimeEventStore`] (shared resume log for WebSocket sessions) and
//! [`RedisOnlinePresence`] (cross-node online presence). Wire them into a host
//! with the existing `HostBuilder::use_realtime_event_store` /
//! `HostBuilder::use_online_presence` entries:
//! `use_realtime_event_store(Arc::new(RedisRealtimeEventStore::new(connection)))`.
//! There is no second registration vocabulary to learn.
//!
//! ```no_run
//! use webx_core::route::ext::IServiceCollectionExt;
//! use webx_host::server::Host;
//! use webx_redis::{RedisHostBuilderExt, RedisOptions};
//!
//! let host = Host::builder()
//!     .add_redis(
//!         RedisOptions::new("redis://127.0.0.1:6379")
//!             .with_prefix("app:")
//!             .with_default_ttl(std::time::Duration::from_secs(300)),
//!     )
//!     .register(|svc| svc.add_mediator())
//!     .build();
//! ```
//!
//! Cache-only services call [`RedisOptions::cache_only`]; publisher-only
//! services set [`RedisEventsOptions::consumer_enabled`] to `false`. The
//! connection is lazy, so host startup does not fail when Redis is temporarily
//! down; `/health/ready` reports the real state.

mod advisory_lock;
mod cache;
mod connection;
mod events;
mod health;
mod options;
mod presence;
mod realtime;

pub use advisory_lock::RedisAdvisoryLock;
pub use cache::RedisCache;
pub use connection::RedisConnection;
pub use events::{RedisEventConsumer, RedisEventPublisher};
pub use health::RedisHealthCheck;
pub use options::{RedisEventsOptions, RedisOptions};
pub use presence::{RedisOnlinePresence, RedisOnlinePresenceOptions};
pub use realtime::{RedisRealtimeEventStore, RedisRealtimeEventStoreOptions};

use std::sync::Arc;

use rust_dix::{IServiceResolverExt, ServiceCollection};
use webx_core::cache::IDistributedCache;
use webx_core::hosted::BackgroundServiceSpec;
use webx_core::locking::IAdvisoryLock;
use webx_core::mediator::{EventDispatcher, IEventPublisher};
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::health::HealthCheckSpec;
use webx_host::server::HostBuilder;

/// `ServiceCollection` registration for [`add_redis`](Self::add_redis).
///
/// Use this inside `HostBuilder::register` when the host-level helper
/// ([`RedisHostBuilderExt`]) is not available (e.g. a manually built provider).
pub trait RedisServiceCollectionExt: Sized {
    /// Register the Redis cache and/or event transport from `options`.
    fn add_redis(self, options: impl Into<RedisOptions>) -> Self;

    /// Register with an already created connection (shared by several
    /// collections/tests).
    fn add_redis_with(self, connection: Arc<RedisConnection>, options: RedisOptions) -> Self;
}

impl RedisServiceCollectionExt for ServiceCollection {
    fn add_redis(self, options: impl Into<RedisOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(RedisConnection::new(
            options.url.clone(),
            options.connect_timeout(),
            options.command_timeout(),
        ));
        self.add_redis_with(connection, options)
    }

    fn add_redis_with(self, connection: Arc<RedisConnection>, options: RedisOptions) -> Self {
        // The connection is a singleton too: real-time ports built on the same
        // deployment (shared resume log / presence) resolve it instead of opening
        // a second one.
        let mut services = self.instance::<RedisConnection>(Arc::clone(&connection));

        if options.cache_enabled {
            let cache = Arc::new(RedisCache::new(Arc::clone(&connection), &options));
            services = services
                .instance::<RedisCache>(Arc::clone(&cache))
                .instance::<dyn IDistributedCache>(cache);
        }

        if let Some(events) = options.events.clone() {
            let publisher = Arc::new(RedisEventPublisher::new(
                Arc::clone(&connection),
                events.clone(),
            ));
            services = services.instance::<dyn IEventPublisher>(publisher);

            if events.consumer_enabled {
                // Register the worker as a singleton *and* start it, so
                // operations code can resolve it for dead-letter replay and
                // consumer introspection.
                let consumer_connection = Arc::clone(&connection);
                let consumer_events = events.clone();
                services = services
                    .try_add_singleton::<RedisEventConsumer>(move |resolver| {
                        let provider = resolver
                            .provider_arc()
                            .expect("ServiceProvider not available from DI resolver");
                        Arc::new(RedisEventConsumer::new(
                            Arc::clone(&consumer_connection),
                            consumer_events.clone(),
                            Arc::new(EventDispatcher::new(provider)),
                        ))
                    })
                    .add_background_service(BackgroundServiceSpec::with_factory(|resolver| {
                        resolver
                            .resolve_arc::<RedisEventConsumer>()
                            .expect("the Redis event consumer is registered above")
                    }));
            }
        }

        services
    }
}

/// `HostBuilder` registration for [`add_redis`](Self::add_redis).
///
/// This is the ASP.NET Core-style entry point: it wires the cache, the event
/// transport, the background consumer and a Redis readiness check in one call.
pub trait RedisHostBuilderExt: Sized {
    /// Register Redis cache/event transport plus `redis` health check.
    fn add_redis(self, options: impl Into<RedisOptions>) -> Self;

    /// Same as [`add_redis`](Self::add_redis), but with a pre-built connection.
    fn add_redis_with(self, connection: Arc<RedisConnection>, options: RedisOptions) -> Self;

    /// Register a Redis advisory lock (`IAdvisoryLock`) at `options.url`.
    ///
    /// The lock shares the URL/prefix conventions of the cache but uses its own
    /// lazily connected handle; keys become `{key_prefix}lock:{key}`.
    fn add_redis_advisory_lock(self, options: impl Into<RedisOptions>) -> Self;
}

impl RedisHostBuilderExt for HostBuilder {
    fn add_redis(self, options: impl Into<RedisOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(RedisConnection::new(
            options.url.clone(),
            options.connect_timeout(),
            options.command_timeout(),
        ));
        self.add_redis_with(connection, options)
    }

    fn add_redis_with(self, connection: Arc<RedisConnection>, options: RedisOptions) -> Self {
        let health_connection = Arc::clone(&connection);
        let service_options = options.clone();
        let builder = self.register(move |services| {
            services.add_redis_with(Arc::clone(&connection), service_options)
        });

        if options.cache_enabled || options.events.is_some() {
            builder.add_health_check(HealthCheckSpec::async_check(RedisHealthCheck::new(
                health_connection,
            )))
        } else {
            builder
        }
    }

    fn add_redis_advisory_lock(self, options: impl Into<RedisOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(RedisConnection::new(
            options.url.clone(),
            options.connect_timeout(),
            options.command_timeout(),
        ));
        let lock: Arc<dyn IAdvisoryLock> =
            Arc::new(RedisAdvisoryLock::new(connection, options.key_prefix));
        self.register(move |services| services.instance::<dyn IAdvisoryLock>(lock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::cache::DistributedCacheEntryOptions;

    #[test]
    fn options_have_aspnet_core_style_defaults() {
        let options = RedisOptions::new("redis://cache:6379/2");
        assert_eq!(options.url, "redis://cache:6379/2");
        assert!(options.cache_enabled);
        let events = options.events.clone().expect("events enabled by default");
        assert!(events.consumer_enabled);
        assert_eq!(events.stream, "webx:events");
        assert!(events.broadcast_channel.is_empty());
        assert_eq!(
            events.effective_broadcast_channel(),
            "webx:events:broadcast"
        );
    }

    #[test]
    fn cache_only_and_publisher_only_toggles_work() {
        let cache_only = RedisOptions::new("redis://cache").cache_only();
        assert!(cache_only.events.is_none());

        let publisher_only = RedisOptions::new("redis://cache")
            .without_cache()
            .with_events(RedisEventsOptions {
                consumer_enabled: false,
                ..RedisEventsOptions::default()
            });
        assert!(!publisher_only.cache_enabled);
        assert!(!publisher_only.events.expect("events").consumer_enabled);
    }

    #[test]
    fn entry_options_are_kept() {
        let options = RedisOptions::new("redis://cache")
            .with_default_ttl(std::time::Duration::from_secs(60))
            .with_sliding_ttl(std::time::Duration::from_secs(30));
        assert_eq!(
            options.default_ttl(),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(
            options.sliding_ttl(),
            Some(std::time::Duration::from_secs(30))
        );
        let _ = DistributedCacheEntryOptions::new()
            .set_absolute_expiration_relative_to_now(std::time::Duration::from_secs(10));
    }
}
