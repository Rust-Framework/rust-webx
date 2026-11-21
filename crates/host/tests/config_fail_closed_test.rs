//! F2 regression: the host refuses to start on a configuration it cannot read
//! exactly, instead of silently falling back to defaults.
//!
//! The test binary contains a single test so the process-global `APP__`
//! variables cannot race with another test.

use webx_core::mode::AppMode;
use webx_host::server::Host;

#[test]
fn host_startup_reads_strict_config_and_rejects_typos() {
    // 1. A valid override is applied and visible on the built host.
    std::env::set_var("APP__Server__RequestTimeoutSeconds", "7");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .try_build()
        .expect("a valid override must build");
    std::env::remove_var("APP__Server__RequestTimeoutSeconds");
    assert_eq!(host.options().server.request_timeout_seconds, 7);

    // 2. A misspelled framework key is a startup error, not a silent default.
    std::env::set_var("APP__Server__RequestTimeoutSecond", "5");
    let result = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .try_build();
    std::env::remove_var("APP__Server__RequestTimeoutSecond");

    let error = match result {
        Ok(_) => panic!("a typo in a framework section must fail startup"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("unknown key"),
        "the error must explain the typo, got: {message}"
    );
    assert!(
        message.contains("RequestTimeoutSecond"),
        "the error must name the offending key, got: {message}"
    );
}
