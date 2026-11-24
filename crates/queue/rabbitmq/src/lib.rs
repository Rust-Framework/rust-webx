//! RabbitMQ (AMQP) event transport for rust-webx.
//!
//! ASP.NET Core style: add the crate, call
//! [`add_rabbitmq`](RabbitMqHostBuilderExt::add_rabbitmq), and events flow
//! through a durable topic exchange / queue. The publisher waits for broker
//! confirms; the consumer acknowledges only after the application handlers ran
//! and requeues failures, so delivery is at-least-once.
//!
//! ```no_run
//! use webx_core::route::ext::IServiceCollectionExt;
//! use webx_host::server::Host;
//! use webx_rabbitmq::{RabbitMqHostBuilderExt, RabbitMqOptions};
//!
//! let host = Host::builder()
//!     .add_rabbitmq(
//!         RabbitMqOptions::new("amqp://guest:guest@127.0.0.1:5672/%2f")
//!             .with_topology("app.events", "user.created", "app.users"),
//!     )
//!     .register(|svc| svc.add_mediator())
//!     .build();
//! ```

mod connection;
mod events;
mod health;
mod options;

pub use connection::RabbitConnection;
pub use events::{RabbitMqEventConsumer, RabbitMqEventPublisher};
pub use health::RabbitMqHealthCheck;
pub use options::RabbitMqOptions;

use std::sync::Arc;

use rust_dix::ServiceCollection;
use webx_core::mediator::{EventDispatcher, IEventPublisher};
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::health::HealthCheckSpec;
use webx_host::server::HostBuilder;

/// `ServiceCollection` registration for [`add_rabbitmq`](Self::add_rabbitmq).
pub trait RabbitMqServiceCollectionExt: Sized {
    /// Register the RabbitMQ publisher (and consumer when enabled).
    fn add_rabbitmq(self, options: impl Into<RabbitMqOptions>) -> Self;

    /// Register with an already created connection.
    fn add_rabbitmq_with(self, connection: Arc<RabbitConnection>, options: RabbitMqOptions)
        -> Self;
}

impl RabbitMqServiceCollectionExt for ServiceCollection {
    fn add_rabbitmq(self, options: impl Into<RabbitMqOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(RabbitConnection::new(
            options.url.clone(),
            options.connect_timeout(),
        ));
        self.add_rabbitmq_with(connection, options)
    }

    fn add_rabbitmq_with(
        self,
        connection: Arc<RabbitConnection>,
        options: RabbitMqOptions,
    ) -> Self {
        let publisher = Arc::new(RabbitMqEventPublisher::new(
            Arc::clone(&connection),
            options.clone(),
        ));
        let mut services = self.instance::<dyn IEventPublisher>(publisher);

        if options.consumer_enabled {
            services = services.add_background_service_with(move |resolver| {
                let provider = resolver
                    .provider_arc()
                    .expect("ServiceProvider not available from DI resolver");
                let dispatcher = Arc::new(EventDispatcher::new(provider));
                Arc::new(RabbitMqEventConsumer::new(
                    Arc::clone(&connection),
                    options.clone(),
                    dispatcher,
                ))
            });
        }

        services
    }
}

/// `HostBuilder` registration for [`add_rabbitmq`](Self::add_rabbitmq).
pub trait RabbitMqHostBuilderExt: Sized {
    /// Register the RabbitMQ publisher/consumer plus a `rabbitmq` health check.
    fn add_rabbitmq(self, options: impl Into<RabbitMqOptions>) -> Self;

    /// Same as [`add_rabbitmq`](Self::add_rabbitmq), but with a pre-built connection.
    fn add_rabbitmq_with(self, connection: Arc<RabbitConnection>, options: RabbitMqOptions)
        -> Self;
}

impl RabbitMqHostBuilderExt for HostBuilder {
    fn add_rabbitmq(self, options: impl Into<RabbitMqOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(RabbitConnection::new(
            options.url.clone(),
            options.connect_timeout(),
        ));
        self.add_rabbitmq_with(connection, options)
    }

    fn add_rabbitmq_with(
        self,
        connection: Arc<RabbitConnection>,
        options: RabbitMqOptions,
    ) -> Self {
        let health_connection = Arc::clone(&connection);
        let service_options = options.clone();
        self.register(move |services| {
            services.add_rabbitmq_with(Arc::clone(&connection), service_options)
        })
        .add_health_check(HealthCheckSpec::async_check(RabbitMqHealthCheck::new(
            health_connection,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_aspnet_core_style() {
        let options = RabbitMqOptions::new("amqp://rabbit:5672/%2f");
        assert_eq!(options.exchange, "webx.events");
        assert_eq!(options.routing_key, "webx.events");
        assert_eq!(options.queue, "webx.events");
        assert!(options.consumer_enabled);
        assert_eq!(options.prefetch, 32);
        assert_eq!(options.max_deliveries, 5);
        assert!(options.dead_letter_exchange.is_empty());
    }

    #[test]
    fn topology_and_publisher_only_are_configurable() {
        let mut options = RabbitMqOptions::new("amqp://rabbit")
            .with_topology("app.events", "user.created", "app.users")
            .publisher_only();
        options.max_deliveries = 3;
        options.dead_letter_exchange = "app.events.dlx".to_string();
        assert_eq!(options.exchange, "app.events");
        assert_eq!(options.routing_key, "user.created");
        assert_eq!(options.queue, "app.users");
        assert!(!options.consumer_enabled);
        assert_eq!(options.max_deliveries, 3);
    }
}
