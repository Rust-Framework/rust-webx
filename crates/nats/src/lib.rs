//! NATS event transport for rust-webx.
//!
//! ASP.NET Core style: add the crate, call [`add_nats`](NatsHostBuilderExt::add_nats),
//! and `IMediator::publish` hands events to NATS. The background consumer routes
//! them through the same `#[handler]` event handlers as the in-memory default.
//!
//! ```no_run
//! use webx_core::route::ext::IServiceCollectionExt;
//! use webx_host::server::Host;
//! use webx_nats::{NatsHostBuilderExt, NatsOptions};
//!
//! let host = Host::builder()
//!     .add_nats(NatsOptions::new("nats://127.0.0.1:4222").with_subject("app.events"))
//!     .register(|svc| svc.add_mediator())
//!     .build();
//! ```
//!
//! Core NATS is at-most-once. Use [`rust-webx-redis`](https://crates.io/crates/rust-webx-redis)
//! or [`rust-webx-rabbitmq`](https://crates.io/crates/rust-webx-rabbitmq) when
//! events must survive a consumer outage.

mod connection;
mod events;
mod health;
mod options;

pub use connection::NatsConnection;
pub use events::{NatsEventConsumer, NatsEventPublisher};
pub use health::NatsHealthCheck;
pub use options::NatsOptions;

use std::sync::Arc;

use rust_dix::ServiceCollection;
use webx_core::mediator::{EventDispatcher, IEventPublisher};
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::server::HostBuilder;

/// `ServiceCollection` registration for [`add_nats`](Self::add_nats).
pub trait NatsServiceCollectionExt: Sized {
    /// Register the NATS event publisher (and consumer when enabled).
    fn add_nats(self, options: impl Into<NatsOptions>) -> Self;

    /// Register with an already created connection.
    fn add_nats_with(self, connection: Arc<NatsConnection>, options: NatsOptions) -> Self;
}

impl NatsServiceCollectionExt for ServiceCollection {
    fn add_nats(self, options: impl Into<NatsOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(NatsConnection::new(
            options.url.clone(),
            options.connection_timeout(),
        ));
        self.add_nats_with(connection, options)
    }

    fn add_nats_with(self, connection: Arc<NatsConnection>, options: NatsOptions) -> Self {
        let publisher = Arc::new(NatsEventPublisher::new(
            Arc::clone(&connection),
            options.subject.clone(),
        ));
        let mut services = self.instance::<dyn IEventPublisher>(publisher);

        if options.consumer_enabled {
            services = services.add_background_service_with(move |resolver| {
                let provider = resolver
                    .provider_arc()
                    .expect("ServiceProvider not available from DI resolver");
                let dispatcher = Arc::new(EventDispatcher::new(provider));
                Arc::new(NatsEventConsumer::new(
                    Arc::clone(&connection),
                    options.clone(),
                    dispatcher,
                ))
            });
        }

        services
    }
}

/// `HostBuilder` registration for [`add_nats`](Self::add_nats).
pub trait NatsHostBuilderExt: Sized {
    /// Register the NATS publisher/consumer plus a `nats` health check.
    fn add_nats(self, options: impl Into<NatsOptions>) -> Self;

    /// Same as [`add_nats`](Self::add_nats), but with a pre-built connection.
    fn add_nats_with(self, connection: Arc<NatsConnection>, options: NatsOptions) -> Self;
}

impl NatsHostBuilderExt for HostBuilder {
    fn add_nats(self, options: impl Into<NatsOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(NatsConnection::new(
            options.url.clone(),
            options.connection_timeout(),
        ));
        self.add_nats_with(connection, options)
    }

    fn add_nats_with(self, connection: Arc<NatsConnection>, options: NatsOptions) -> Self {
        let health_connection = Arc::clone(&connection);
        let service_options = options.clone();
        self.register(move |services| {
            services.add_nats_with(Arc::clone(&connection), service_options)
        })
        .add_health_check_async(NatsHealthCheck::new(health_connection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_aspnet_core_style() {
        let options = NatsOptions::new("nats://nats:4222");
        assert_eq!(options.url, "nats://nats:4222");
        assert_eq!(options.subject, "webx.events");
        assert!(options.consumer_enabled);
        assert!(options.queue_group.is_none());
    }

    #[test]
    fn publisher_only_disables_the_consumer() {
        let options = NatsOptions::new("nats://nats")
            .with_queue_group("workers")
            .publisher_only();
        assert!(!options.consumer_enabled);
        assert_eq!(options.queue_group.as_deref(), Some("workers"));
    }
}
