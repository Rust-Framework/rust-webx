//! Kafka event transport for rust-webx.
//!
//! ASP.NET Core style: add the crate, call [`add_kafka`](KafkaHostBuilderExt::add_kafka),
//! and events flow through Kafka. This adapter is deliberately a **small,
//! single-partition follower** built on the pure-Rust `rskafka` client (no
//! librdkafka/C toolchain): the framework port is `IEventPublisher` +
//! `EventDispatcher`, and a richer group-aware/TLS/SASL adapter can replace it
//! without touching core. This starter connects to plaintext `host:port`
//! brokers only.
//!
//! ```no_run
//! use webx_core::route::ext::IServiceCollectionExt;
//! use webx_host::server::Host;
//! use webx_kafka::{KafkaHostBuilderExt, KafkaOptions, KafkaStartOffset};
//!
//! let host = Host::builder()
//!     .add_kafka(
//!         KafkaOptions::new("127.0.0.1:9092", "app.events")
//!             .with_partition(0)
//!             .with_start_offset(KafkaStartOffset::Earliest),
//!     )
//!     .register(|svc| svc.add_mediator())
//!     .build();
//! ```

mod connection;
mod events;
mod health;
mod options;

pub use connection::KafkaConnection;
pub use events::{KafkaEventConsumer, KafkaEventPublisher};
pub use health::KafkaHealthCheck;
pub use options::{KafkaOptions, KafkaStartOffset};

use std::sync::Arc;

use rust_dix::ServiceCollection;
use webx_core::mediator::{EventDispatcher, IEventPublisher};
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::server::HostBuilder;

/// `ServiceCollection` registration for [`add_kafka`](Self::add_kafka).
pub trait KafkaServiceCollectionExt: Sized {
    /// Register the Kafka publisher (and consumer when enabled).
    fn add_kafka(self, options: impl Into<KafkaOptions>) -> Self;

    /// Register with an already created connection.
    fn add_kafka_with(self, connection: Arc<KafkaConnection>, options: KafkaOptions) -> Self;
}

impl KafkaServiceCollectionExt for ServiceCollection {
    fn add_kafka(self, options: impl Into<KafkaOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(KafkaConnection::new(
            options.brokers.clone(),
            options.connect_timeout(),
        ));
        self.add_kafka_with(connection, options)
    }

    fn add_kafka_with(self, connection: Arc<KafkaConnection>, options: KafkaOptions) -> Self {
        let publisher = Arc::new(KafkaEventPublisher::new(
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
                Arc::new(KafkaEventConsumer::new(
                    Arc::clone(&connection),
                    options.clone(),
                    dispatcher,
                ))
            });
        }

        services
    }
}

/// `HostBuilder` registration for [`add_kafka`](Self::add_kafka).
pub trait KafkaHostBuilderExt: Sized {
    /// Register the Kafka publisher/consumer plus a `kafka` health check.
    fn add_kafka(self, options: impl Into<KafkaOptions>) -> Self;

    /// Same as [`add_kafka`](Self::add_kafka), but with a pre-built connection.
    fn add_kafka_with(self, connection: Arc<KafkaConnection>, options: KafkaOptions) -> Self;
}

impl KafkaHostBuilderExt for HostBuilder {
    fn add_kafka(self, options: impl Into<KafkaOptions>) -> Self {
        let options = options.into();
        let connection = Arc::new(KafkaConnection::new(
            options.brokers.clone(),
            options.connect_timeout(),
        ));
        self.add_kafka_with(connection, options)
    }

    fn add_kafka_with(self, connection: Arc<KafkaConnection>, options: KafkaOptions) -> Self {
        let health_connection = Arc::clone(&connection);
        let service_options = options.clone();
        self.register(move |services| {
            services.add_kafka_with(Arc::clone(&connection), service_options)
        })
        .add_health_check_async(KafkaHealthCheck::new(health_connection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_aspnet_core_style() {
        let options = KafkaOptions::new("kafka:9092", "app.events");
        assert_eq!(options.brokers, vec!["kafka:9092".to_string()]);
        assert_eq!(options.topic, "app.events");
        assert_eq!(options.partition, 0);
        assert!(options.consumer_enabled);
        assert_eq!(options.start_offset, KafkaStartOffset::Latest);
        assert!(options.create_topic);
    }

    #[test]
    fn publisher_only_and_partition_are_configurable() {
        let options = KafkaOptions::new("kafka:9092", "app.events")
            .with_partition(3)
            .with_start_offset(KafkaStartOffset::Earliest)
            .publisher_only();
        assert_eq!(options.partition, 3);
        assert_eq!(options.start_offset, KafkaStartOffset::Earliest);
        assert!(!options.consumer_enabled);
    }
}
