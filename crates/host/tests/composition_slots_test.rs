//! The composition root has exactly two slots: `register` (DI object graph) and
//! `configure` (`AppOptions`).
//!
//! Typed helpers may add sugar, but these two slots must stay sufficient and
//! order-independent. This test pins that contract so a future builder helper
//! cannot quietly introduce a third assembly channel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use webx_core::metrics::{IMeter, MemoryMeter};
use webx_core::options_monitor::{IOptionsMonitor, OptionsMonitor};
use webx_core::security::{ISessionStore, InMemorySessionStore};
use webx_core::time::TestClock;
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
        app.options(|options| {
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
            app.options(|options| options.app.max_body_size = 1);
            CONFIGURE_CALLS.fetch_add(1, Ordering::SeqCst);
        })
        .configure(|app| {
            app.options(|options| options.app.max_body_size = 2);
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
        .configure(|app| app.options(|options| options.app.max_body_size = 64))
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

// ---------------------------------------------------------------------------
// Typed options helpers: `add_options` / `add_options_with`
// ---------------------------------------------------------------------------

#[derive(Debug, Default, serde::Deserialize)]
struct SiteConfig {
    #[serde(default)]
    site_name: String,
    #[serde(default)]
    max_connections: u32,
}

#[test]
fn add_options_with_applies_the_override_before_every_registration_form() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_options_with::<SiteConfig>("Site", |site| {
            site.site_name = "code-override".to_string();
            site.max_connections = 42;
        })
        .build();
    let provider = host.provider();

    let snapshot = provider
        .get::<SiteConfig>()
        .expect("`Arc<T>` is registered");
    assert_eq!(snapshot.site_name, "code-override");
    assert_eq!(snapshot.max_connections, 42);

    let monitor = provider
        .get::<OptionsMonitor<SiteConfig>>()
        .expect("`Arc<OptionsMonitor<T>>` is registered");
    assert_eq!(monitor.current().site_name, "code-override");
    assert_eq!(monitor.current().max_connections, 42);

    let port = provider
        .get::<dyn IOptionsMonitor<SiteConfig>>()
        .expect("`Arc<dyn IOptionsMonitor<T>>` is registered");
    assert_eq!(port.current().site_name, "code-override");
    assert_eq!(port.current().max_connections, 42);
    assert!(
        Arc::ptr_eq(&monitor.current(), &port.current()),
        "the port must observe the registered monitor"
    );
}

#[test]
fn the_last_use_override_wins_for_a_single_port() {
    let first: Arc<dyn webx_core::time::IClock> = Arc::new(TestClock::from_unix(1_000));
    let second: Arc<dyn webx_core::time::IClock> = Arc::new(TestClock::from_unix(2_000));
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_clock(first)
        .use_clock(second)
        .build();

    assert_eq!(
        host.clock().now_unix(),
        2_000,
        "`use_*` registers one instance; the last call wins"
    );
}

#[test]
fn a_later_configure_write_wins_over_an_earlier_one() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| app.options(|o| o.app.max_body_size = 1))
        .configure(|app| app.options(|o| o.app.max_body_size = 2))
        .build();

    assert_eq!(host.options().app.max_body_size, 2);
}

// ---------------------------------------------------------------------------
// Port replacement: `use_*` registers one instance and the last call wins.
//
// Service configs run in call order, so the port registration order is the
// builder call order. `instance` appends and rust-dix resolves the last
// registration; the build-time defaults use `try_add_*` and only fill in when
// the application registered nothing.
// ---------------------------------------------------------------------------

#[test]
fn the_last_use_meter_call_wins() {
    let first: Arc<dyn IMeter> = Arc::new(MemoryMeter::new());
    let second: Arc<dyn IMeter> = Arc::new(MemoryMeter::new());
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_meter(Arc::clone(&first))
        .use_meter(Arc::clone(&second))
        .build();

    let resolved = host
        .provider()
        .get::<dyn IMeter>()
        .expect("`use_meter` registers `dyn IMeter`");
    assert!(
        Arc::ptr_eq(&resolved, &second),
        "`use_meter` replaces the port: the last call must win"
    );
    assert!(
        !Arc::ptr_eq(&resolved, &first),
        "the first `use_meter` instance must not stay authoritative"
    );
}

#[test]
fn use_meter_replaces_an_instance_registered_before_it() {
    let registered: Arc<dyn IMeter> = Arc::new(MemoryMeter::new());
    let used: Arc<dyn IMeter> = Arc::new(MemoryMeter::new());
    let registered_for_closure = Arc::clone(&registered);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(move |svc| svc.instance::<dyn IMeter>(registered_for_closure))
        .use_meter(Arc::clone(&used))
        .build();

    let resolved = host
        .provider()
        .get::<dyn IMeter>()
        .expect("`use_meter` registers `dyn IMeter`");
    assert!(
        Arc::ptr_eq(&resolved, &used),
        "`use_*` called after a manual registration must win"
    );
}

#[test]
fn the_last_use_session_store_call_wins() {
    let first: Arc<dyn ISessionStore> = Arc::new(InMemorySessionStore::new());
    let second: Arc<dyn ISessionStore> = Arc::new(InMemorySessionStore::new());
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_session_store(Arc::clone(&first))
        .use_session_store(Arc::clone(&second))
        .build();

    let resolved = host
        .provider()
        .get::<dyn ISessionStore>()
        .expect("`use_session_store` registers `dyn ISessionStore`");
    assert!(
        Arc::ptr_eq(&resolved, &second),
        "`use_session_store` replaces the port: the last call must win"
    );
    assert!(
        !Arc::ptr_eq(&resolved, &first),
        "the first `use_session_store` instance must not stay authoritative"
    );
}
