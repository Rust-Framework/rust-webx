//! Strict/lenient scope validation on `HostBuilder` (rust-dix 0.8+ semantics).

use std::sync::Arc;

use webx_core::mode::AppMode;
use webx_host::server::Host;

#[derive(Debug)]
struct ScopedThing(u32);

fn ready_builder() -> webx_host::server::HostBuilder {
    Host::builder().mode(AppMode::Development).no_spa()
}

#[test]
fn strict_scope_validation_is_the_default() {
    let host = ready_builder()
        .register(|svc| svc.scoped::<ScopedThing>(|_| Arc::new(ScopedThing(1))))
        .try_build()
        .expect("host must build");

    let error = host
        .provider()
        .get::<ScopedThing>()
        .expect_err("strict scope validation must reject Scoped services from the root");
    let message = error.to_string();
    assert!(
        message.contains("root provider"),
        "the error must explain the root-scope rule, got: {message}"
    );

    // The supported path: open a scope and resolve there.
    let scope = host.provider().scope();
    assert!(
        scope.get::<ScopedThing>().is_ok(),
        "resolving inside an explicit scope must work"
    );
}

#[test]
fn lenient_scope_validation_restores_root_resolution() {
    let host = ready_builder()
        .lenient_scope_validation()
        .register(|svc| svc.scoped::<ScopedThing>(|_| Arc::new(ScopedThing(7))))
        .try_build()
        .expect("host must build");

    let first = host
        .provider()
        .get::<ScopedThing>()
        .expect("lenient mode resolves Scoped services from the root");
    let second = host
        .provider()
        .get::<ScopedThing>()
        .expect("the root-local instance stays resolvable");
    assert!(
        Arc::ptr_eq(&first, &second),
        "lenient root resolution caches the instance root-locally, like rust-dix 0.7"
    );
    assert_eq!(first.0, 7);
}
