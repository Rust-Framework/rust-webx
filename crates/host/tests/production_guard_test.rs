//! Production-mode build guards (fail-fast).

use webx_core::mode::AppMode;
use webx_host::server::Host;

fn production_options(app: &mut webx_host::server::HostAppBuilder) {
    app.useOptions(|o| {
        o.cors.origins = vec!["https://example.com".into()];
    });
}

#[test]
#[should_panic(expected = "Production requires a strong JWT secret")]
fn production_build_rejects_weak_jwt_when_auth_enabled() {
    Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(production_options)
        .add_authentication()
        .build();
}

#[test]
#[should_panic(expected = "Production forbids CORS origin '*'")]
fn production_build_rejects_wildcard_cors() {
    // Explicit "*" — do not rely on missing appsettings (cwd may discover
    // docbit Production overlay with concrete origins after Cors PascalCase bind fix).
    Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.useOptions(|o| {
                o.cors.origins = vec!["*".into()];
            });
        })
        .build();
}

#[test]
fn production_build_succeeds_with_strong_jwt_and_explicit_cors() {
    Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.useOptions(|o| {
                o.cors.origins = vec!["https://example.com".into()];
                o.jwt.secret = "production-jwt-secret-at-least-32-chars".into();
            });
        })
        .add_authentication()
        .build();
}

// --- try_build: the same guards as `Err` instead of a panic (0.10) ---

#[test]
fn try_build_returns_err_instead_of_panicking() {
    // 1) Production + weak/missing JWT secret with authentication enabled.
    let error = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(production_options)
        .add_authentication()
        .try_build()
        .err()
        .expect("weak JWT in Production must be rejected");
    assert!(
        matches!(error, webx_core::error::Error::Di(_)),
        "expected Error::Di, got {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("Production requires a strong JWT secret"),
        "got {error}"
    );

    // 2) Production + wildcard CORS.
    let error = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.useOptions(|o| {
                o.cors.origins = vec!["*".into()];
                o.jwt.secret = "production-jwt-secret-at-least-32-chars".into();
            });
        })
        .try_build()
        .err()
        .expect("wildcard CORS in Production must be rejected");
    assert!(
        error
            .to_string()
            .contains("Production forbids CORS origin '*'"),
        "got {error}"
    );
}

#[test]
fn try_build_succeeds() {
    let host = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.useOptions(|o| {
                o.cors.origins = vec!["https://example.com".into()];
                o.jwt.secret = "production-jwt-secret-at-least-32-chars".into();
            });
        })
        .add_authentication()
        .try_build()
        .expect("a valid Production configuration must build");

    // The accepted configuration is what the host keeps, and the built host is
    // fully usable (provider + per-host dispatch runtime).
    assert_eq!(
        host.options().cors.origins,
        vec!["https://example.com".to_string()]
    );
    assert_eq!(
        host.options().jwt.secret,
        "production-jwt-secret-at-least-32-chars"
    );
    assert!(
        host.provider()
            .get::<dyn webx_core::localization::IStringLocalizer>()
            .is_ok(),
        "host services are registered in the built provider"
    );
    let _ = host.dispatch_runtime();
}
