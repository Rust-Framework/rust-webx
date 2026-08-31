//! Production-mode build guards (fail-fast).

use rust_webx_core::mode::AppMode;
use rust_webx_host::server::Host;

fn production_options(app: &mut rust_webx_host::server::HostAppBuilder) {
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
