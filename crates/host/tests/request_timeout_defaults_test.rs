//! F3 regression: the inbound request deadline is bounded by default and
//! Production cannot turn it off.
//!
//! The test binary contains a single test so the process-global `APP__`
//! variables cannot race with another test.

use webx_core::mode::AppMode;
use webx_host::server::Host;

fn build(mode: AppMode) -> Result<Host, String> {
    Host::builder()
        .mode(mode)
        .no_spa()
        .try_build()
        .map_err(|error| error.to_string())
}

#[test]
fn request_deadline_defaults_to_30s_and_production_rejects_zero() {
    // Development: the default is a bounded 30s (not "disabled").
    let dev = build(AppMode::Development).expect("development host builds");
    assert_eq!(dev.options().server.request_timeout_seconds, 30);
    drop(dev);

    // Development: explicit 0 is the documented escape hatch.
    std::env::set_var("APP__Server__RequestTimeoutSeconds", "0");
    let dev_disabled = build(AppMode::Development).expect("development may disable the deadline");
    assert_eq!(dev_disabled.options().server.request_timeout_seconds, 0);
    drop(dev_disabled);

    // Production needs explicit CORS to clear the *other* production guard.
    std::env::set_var("APP__Cors__Origins", r#"["https://example.com"]"#);

    // Production: 0 is fail-closed.
    let error = match build(AppMode::Production) {
        Ok(_) => panic!("production must reject a disabled inbound deadline"),
        Err(error) => error,
    };
    assert!(
        error.contains("RequestTimeoutSeconds"),
        "the error must name the knob, got: {error}"
    );

    // Production: without the override the 30s default applies.
    std::env::remove_var("APP__Server__RequestTimeoutSeconds");
    let prod = build(AppMode::Production).expect("production builds with the default deadline");
    assert_eq!(prod.options().server.request_timeout_seconds, 30);

    std::env::remove_var("APP__Cors__Origins");
}
