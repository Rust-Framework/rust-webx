//! The composition root has exactly two slots: `register` (DI object graph) and
//! `configure` (`AppOptions`).
//!
//! Typed helpers may add sugar, but these two slots must stay sufficient and
//! order-independent. This test pins that contract so a future builder helper
//! cannot quietly introduce a third assembly channel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use webx_core::{AppMode, AppOptions};
use webx_host::{Host, HostBuilder};

#[derive(Debug)]
struct Marker(usize);

#[derive(Debug)]
struct SecondMarker(usize);

fn register_slot(builder: HostBuilder) -> HostBuilder {
    builder.register(|svc| svc.singleton(|_| Arc::new(Marker(42))))
}

fn configure_slot(builder: HostBuilder) -> HostBuilder {
    builder.configure(|app| {
        app.use_options(|options| {
            options.metrics.enabled = true;
            options.app.max_body_size = 4096;
        });
    })
}

#[test]
fn configure_and_register_compose_in_either_order() {
    let register_first = register_slot(configure_slot(
        Host::builder().mode(AppMode::Development).no_spa(),
    ));
    let register_last = configure_slot(register_slot(
        Host::builder().mode(AppMode::Development).no_spa(),
    ));

    for host in [register_first.build(), register_last.build()] {
        assert!(
            host.options().metrics.enabled,
            "configure slot must be applied"
        );
        assert_eq!(
            host.options().app.max_body_size,
            4096,
            "configure slot must be applied"
        );
        let marker = host
            .provider()
            .get::<Marker>()
            .expect("register slot must be resolvable");
        assert_eq!(marker.0, 42);
    }
}

#[test]
fn each_slot_runs_every_closure_once_and_in_order() {
    static CONFIGURE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REGISTER_CALLS: AtomicUsize = AtomicUsize::new(0);

    let first = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.use_options(|options| options.app.max_body_size = 1);
            CONFIGURE_CALLS.fetch_add(1, Ordering::SeqCst);
        })
        .configure(|app| {
            app.use_options(|options| options.app.max_body_size = 2);
            CONFIGURE_CALLS.fetch_add(1, Ordering::SeqCst);
        })
        .register(|svc| {
            REGISTER_CALLS.fetch_add(1, Ordering::SeqCst);
            svc.singleton(|_| Arc::new(Marker(1)))
        })
        .register(|svc| {
            REGISTER_CALLS.fetch_add(1, Ordering::SeqCst);
            svc.singleton(|_| Arc::new(SecondMarker(2)))
        })
        .build();

    // Configure closures are applied in chain order: the last write wins.
    assert_eq!(first.options().app.max_body_size, 2);
    assert_eq!(CONFIGURE_CALLS.load(Ordering::SeqCst), 2);
    assert_eq!(REGISTER_CALLS.load(Ordering::SeqCst), 2);

    // Both register closures contribute to the same object graph.
    let provider = first.provider();
    assert_eq!(provider.get::<Marker>().expect("first service").0, 1);
    assert_eq!(provider.get::<SecondMarker>().expect("second service").0, 2);
}

#[test]
fn builder_defaults_stay_valid_when_only_one_slot_is_used() {
    // configure-only: the DI graph keeps the framework defaults.
    let configure_only = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| app.use_options(|options| options.app.max_body_size = 64))
        .build();
    assert_eq!(configure_only.options().app.max_body_size, 64);

    // register-only: options keep the framework defaults.
    let register_only = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| svc.singleton(|_| Arc::new(Marker(7))))
        .build();
    assert_eq!(
        register_only.options().app.max_body_size,
        AppOptions::default().app.max_body_size
    );
    assert_eq!(
        register_only
            .provider()
            .get::<Marker>()
            .expect("registered service")
            .0,
        7
    );
}
