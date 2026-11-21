//! `add_kafka` wires the event publisher without requiring a live broker.

use webx_core::mediator::IEventPublisher;
use webx_core::mode::AppMode;
use webx_host::server::Host;
use webx_kafka::{KafkaHostBuilderExt, KafkaOptions, KafkaServiceCollectionExt};

#[test]
fn add_kafka_registers_an_event_publisher_lazily() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .add_kafka(KafkaOptions::new("127.0.0.1:1", "webx.test.events"))
        .try_build()
        .expect("host builds without connecting to Kafka");

    assert!(host.provider().get::<dyn IEventPublisher>().is_ok());
}

#[test]
fn service_collection_registration_is_usable_from_register() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .register(|services| {
            services
                .add_kafka(KafkaOptions::new("127.0.0.1:1", "webx.test.events").publisher_only())
        })
        .try_build()
        .expect("host builds");

    assert!(host.provider().get::<dyn IEventPublisher>().is_ok());
}
