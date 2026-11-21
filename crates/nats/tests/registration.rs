//! `add_nats` wires the event publisher without requiring a live server.

use webx_core::mediator::IEventPublisher;
use webx_core::mode::AppMode;
use webx_host::server::Host;
use webx_nats::{NatsHostBuilderExt, NatsOptions, NatsServiceCollectionExt};

#[test]
fn add_nats_registers_an_event_publisher_lazily() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .add_nats(NatsOptions::new("nats://127.0.0.1:1"))
        .try_build()
        .expect("host builds without connecting to NATS");

    assert!(host.provider().get::<dyn IEventPublisher>().is_ok());
}

#[test]
fn service_collection_registration_is_usable_from_register() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .register(|services| {
            services.add_nats(NatsOptions::new("nats://127.0.0.1:1").publisher_only())
        })
        .try_build()
        .expect("host builds");

    assert!(host.provider().get::<dyn IEventPublisher>().is_ok());
}
