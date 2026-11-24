//! Startup configuration aggregation (2.8): `HostBuilder::try_build` reports
//! **every** invalid configuration value in one `Error::di`, deterministically
//! ordered, instead of stopping at the first `?`.

use webx_core::config::{AppOptions, TlsClientAuth};
use webx_core::mode::AppMode;
use webx_host::config_validation::{format_violations, validate_options, ConfigViolation};
use webx_host::cors::CorsConfig;
use webx_host::server::Host;

/// Valid Production defaults must not produce a single violation.
#[test]
fn secure_defaults_validate_cleanly() {
    let options = AppOptions::default();
    assert_eq!(validate_options(&options, AppMode::Production), Vec::new());
    assert_eq!(validate_options(&options, AppMode::Development), Vec::new());
}

/// Three independent invalid values are all reported, sorted by section + key.
#[test]
fn validate_options_reports_all_violations_in_stable_order() {
    let mut options = AppOptions::default();
    options.server.request_timeout_seconds = 0;
    options.server.admission_timeout_ms = 0;
    // Half-configured mTLS: ClientAuth requires a CA path.
    options.tls.client_auth = TlsClientAuth::Required;

    let violations = validate_options(&options, AppMode::Production);
    assert_eq!(
        violations.len(),
        3,
        "every invalid key must be reported, got {violations:?}"
    );

    let keys: Vec<&str> = violations.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "Server.AdmissionTimeoutMs",
            "Server.RequestTimeoutSeconds",
            "Tls.ClientAuth",
        ],
        "violations must sort deterministically by section then key"
    );
    assert!(violations[0].message.contains("AdmissionTimeoutMs=0"));
    assert!(violations[1].message.contains("RequestTimeoutSeconds=0"));
    assert!(violations[2]
        .message
        .contains("Tls.ClientAuth requires Tls.ClientCaPath"));

    // `Display` is the single-line `Key: reason` form the aggregate report uses.
    let violation: &ConfigViolation = &violations[2];
    assert_eq!(
        violation.to_string(),
        format!("{}: {}", violation.key, violation.message)
    );

    // The aggregate message is stable across calls (sorted by section + key).
    let report = format_violations(&violations);
    assert_eq!(report, format_violations(&violations));
    assert_eq!(report.matches("\n  - ").count(), 3, "{report}");
    let admission = report.find("Server.AdmissionTimeoutMs").expect("admission");
    let request = report
        .find("Server.RequestTimeoutSeconds")
        .expect("request timeout");
    let tls = report.find("Tls.ClientAuth").expect("mtls");
    assert!(admission < request && request < tls, "{report}");
}

/// Development may switch the inbound deadline off and still configure the
/// legacy `RateLimit` trust keys (warning only); Production rejects the keys.
#[test]
fn mode_specific_exemptions_are_not_flagged() {
    let mut options = AppOptions::default();
    options.server.request_timeout_seconds = 0;
    options.server.admission_timeout_ms = 0;
    options.rate_limit.trust_proxy = true;
    options.rate_limit.trusted_proxies = vec!["10.0.0.0/8".to_string()];

    assert_eq!(
        validate_options(&options, AppMode::Development),
        Vec::new(),
        "Development keeps the unbounded server limits and the migration warning"
    );

    let production = validate_options(&options, AppMode::Production);
    let keys: Vec<&str> = production.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "RateLimit.TrustProxy",
            "Server.AdmissionTimeoutMs",
            "Server.RequestTimeoutSeconds",
        ],
        "got {production:?}"
    );
}

/// `App.MaxConnections` must stay inside the engine's semaphore range; the
/// documented `0 -> 1` clamp is not a startup failure.
#[test]
fn max_connections_is_bounded_but_zero_is_clamped() {
    let mut options = AppOptions::default();
    options.app.max_connections = usize::MAX;
    let violations = validate_options(&options, AppMode::Production);
    assert_eq!(violations.len(), 1, "got {violations:?}");
    assert_eq!(violations[0].key, "App.MaxConnections");
    assert!(
        violations[0]
            .message
            .contains("outside the supported range"),
        "got {}",
        violations[0].message
    );

    options.app.max_connections = 0;
    assert_eq!(
        validate_options(&options, AppMode::Production),
        Vec::new(),
        "0 is the engine's documented clamp to 1, not a violation"
    );
}

/// `try_build` fails once with every violation instead of only the first one.
#[test]
fn try_build_reports_every_violation_at_once() {
    let error = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.use_options(|options| {
                options.server.request_timeout_seconds = 0;
                options.server.admission_timeout_ms = 0;
                options.tls.client_auth = TlsClientAuth::Required;
            });
        })
        .try_build()
        .err()
        .expect("invalid Production configuration must fail startup");

    assert!(
        error.code() == "dependency_injection",
        "expected Error::di, got {error:?}"
    );
    let message = error.to_string();
    assert_eq!(
        message.matches("\n  - ").count(),
        3,
        "all three violations must be in the one error, got: {message}"
    );
    for expected in [
        "Server.RequestTimeoutSeconds=0",
        "Server.AdmissionTimeoutMs=0",
        "Tls.ClientAuth requires Tls.ClientCaPath",
    ] {
        assert!(
            message.contains(expected),
            "missing `{expected}` in: {message}"
        );
    }
    let admission = message
        .find("Server.AdmissionTimeoutMs")
        .expect("admission");
    let request = message
        .find("Server.RequestTimeoutSeconds")
        .expect("request timeout");
    let tls = message.find("Tls.ClientAuth").expect("mtls");
    assert!(
        admission < request && request < tls,
        "the aggregate must be sorted by section + key, got: {message}"
    );
}

/// Builder-state guards (an explicit `use_cors` override, JWT only when
/// authentication is enabled, TURN section) are aggregated too.
#[test]
fn builder_context_guards_are_aggregated() {
    // Wildcard CORS through `use_cors` + an invalid server bound.
    let cors_error = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.use_options(|options| {
                options.server.request_timeout_seconds = 0;
            });
        })
        .use_cors(CorsConfig {
            origins: vec!["*".to_string()],
            ..CorsConfig::default()
        })
        .try_build()
        .err()
        .expect("the explicit override must be validated too");
    let message = cors_error.to_string();
    assert!(
        message.contains("Production forbids CORS origin '*'"),
        "got: {message}"
    );
    assert!(
        message.contains("Server.RequestTimeoutSeconds=0"),
        "got: {message}"
    );

    // Weak JWT secret (authentication enabled) + an invalid server bound.
    let jwt_error = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.use_options(|options| {
                options.server.admission_timeout_ms = 0;
            });
        })
        .use_authentication()
        .try_build()
        .err()
        .expect("weak JWT secret in Production must fail startup");
    let message = jwt_error.to_string();
    assert!(
        message.contains("Production requires a strong JWT secret"),
        "got: {message}"
    );
    assert!(
        message.contains("Server.AdmissionTimeoutMs=0"),
        "got: {message}"
    );

    // Invalid Jwt section only matters when authentication is enabled.
    let jwt_section_error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.use_options(|options| {
                options.jwt.algorithms = Vec::new();
            });
        })
        .use_authentication()
        .try_build()
        .err()
        .expect("an invalid Jwt section must fail startup when auth is on");
    assert!(
        jwt_section_error
            .to_string()
            .contains("Jwt.Algorithms must not be empty"),
        "got: {jwt_section_error}"
    );

    // A partially configured `Turn` section is a startup error, not a silently
    // disabled feature.
    let turn_error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.use_options(|options| {
                options.turn.secret = "shared-secret".to_string();
            });
        })
        .try_build()
        .err()
        .expect("a half-configured Turn section must fail startup");
    assert!(
        turn_error.to_string().contains("Turn.Urls"),
        "got: {turn_error}"
    );
}

/// `build()` panics with exactly the aggregate `try_build` message.
#[test]
fn build_panics_with_the_same_aggregate_message() {
    fn invalid() -> webx_host::server::HostBuilder {
        Host::builder()
            .mode(AppMode::Production)
            .no_spa()
            .configure(|app| {
                app.use_options(|options| {
                    options.server.request_timeout_seconds = 0;
                    options.server.admission_timeout_ms = 0;
                });
            })
    }

    let expected = match invalid().try_build().err() {
        Some(error) => error.operator().unwrap_or_default().to_string(),
        other => panic!("expected Error::di, got {other:?}"),
    };

    let panic = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invalid().build())) {
        Ok(_) => panic!("build() must panic on an invalid configuration"),
        Err(payload) => payload,
    };
    let text = if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_string()
    } else {
        String::new()
    };
    assert_eq!(
        text, expected,
        "build() must panic with try_build's message"
    );
}

/// An enabled limiter with a zero burst or rate is a startup error.
///
/// `BurstSize = 0` makes every request `429` forever and `RequestsPerSecond = 0`
/// stops serving after one burst; both used to start "successfully" and look
/// healthy in the log line.
#[test]
fn enabled_rate_limit_with_zero_limits_is_a_violation() {
    let mut options = AppOptions::default();
    options.rate_limit.enabled = true;
    options.rate_limit.burst_size = 0;
    options.rate_limit.requests_per_second = 0.0;

    let violations = validate_options(&options, AppMode::Development);
    let keys: Vec<&str> = violations.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["RateLimit.BurstSize", "RateLimit.RequestsPerSecond"],
        "both zero values must be reported, sorted by key"
    );

    // Disabling the limiter makes the same numbers irrelevant.
    options.rate_limit.enabled = false;
    assert!(validate_options(&options, AppMode::Development).is_empty());
}

/// `*` plus credentials is refused in every mode, not just Production.
#[test]
fn cors_wildcard_with_credentials_is_always_a_violation() {
    let mut options = AppOptions::default();
    options.cors.origins = vec!["*".to_string()];
    options.cors.allow_credentials = true;

    for mode in [AppMode::Development, AppMode::Production] {
        let violations = validate_options(&options, mode);
        assert!(
            violations.iter().any(|v| v.key == "Cors.Origins"),
            "{mode:?} must refuse '*' with credentials, got {violations:?}"
        );
    }
}

/// A CSRF session-cookie name that differs from the registered cookie scheme is
/// refused at startup: the check would otherwise never run.
#[test]
fn csrf_cookie_name_must_match_the_session_cookie_scheme() {
    use std::sync::Arc;
    use webx_core::security::InMemorySessionRepository;
    use webx_host::auth_cookie::{CookieOptions, SessionCookieScheme};
    use webx_host::csrf::CsrfConfig;

    let options = CookieOptions {
        name: "sid".to_string(),
        ..CookieOptions::default()
    };
    let scheme = SessionCookieScheme::new(Arc::new(InMemorySessionRepository::new()), options)
        .expect("valid cookie options");

    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(Arc::new(scheme))
        .use_csrf(CsrfConfig::default())
        .try_build()
        .err()
        .expect("a drifted CSRF cookie name must abort startup");
    assert!(error.to_string().contains("does not match"), "got {error}");

    // The matching name builds: default CSRF config reads `__Host-webx_session`.
    let matching = SessionCookieScheme::new(
        Arc::new(InMemorySessionRepository::new()),
        CookieOptions::default(),
    )
    .expect("valid cookie options");
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(Arc::new(matching))
        .use_csrf(CsrfConfig::default())
        .try_build()
        .expect("matching cookie names build");
}
