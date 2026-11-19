//! WEBX-01: the resolved JWT signing secret reaches application service
//! registrations.
//!
//! `Jwt.SecretFile` is resolved before user service configurations run, so a
//! custom `ITokenIssuer` can sign with exactly the key the host's JWT middleware
//! verifies — no empty-key signatures in a `SecretFile`-only deployment.

mod test_utils;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jsonwebtoken::{DecodingKey, Validation};
use rust_dix::IServiceResolverExt as _;
use webx_core::auth::IAuthenticationHandler;
use webx_core::http::IClaimsExt;
use webx_core::middleware::IMiddleware;
use webx_core::mode::AppMode;
use webx_core::security::{ITokenIssuer, TokenClaims};
use webx_host::auth_jwt::{jwt_middleware, JwtAuth, JwtSecret, JwtTokenIssuer};
use webx_host::server::{Host, HostAppBuilder};

const GOOD_SECRET: &str = "0123456789abcdef0123456789abcdef"; // exactly 32 bytes

fn write_secret_file(dir: &tempfile::TempDir, content: &[u8]) -> std::path::PathBuf {
    let path = dir.path().join("jwt.key");
    std::fs::write(&path, content).expect("the secret file is writable");
    // Secret files must stay private; on Unix the reader enforces 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("the secret file mode is settable");
    }
    path
}

/// Configure only `Jwt.SecretFile`; `Jwt.Secret` is cleared so a development
/// appsettings override in the workspace cannot mask the file.
fn configure_secret_file(
    path: std::path::PathBuf,
) -> impl FnOnce(&mut HostAppBuilder) + Send + 'static {
    move |app| {
        app.useOptions(move |options| {
            options.jwt.secret.clear();
            options.jwt.secret_file = Some(path.to_string_lossy().into_owned());
        });
    }
}

#[test]
fn register_with_options_receives_the_file_backed_secret() {
    let dir = tempfile::tempdir().expect("a temporary directory is available");
    let path = write_secret_file(&dir, GOOD_SECRET.as_bytes());

    let seen = Arc::new(Mutex::new(None));
    let seen_in_config = Arc::clone(&seen);

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(configure_secret_file(path))
        .register_with_options(move |svc, options| {
            *seen_in_config
                .lock()
                .expect("the observations mutex is not poisoned") =
                Some(options.jwt.secret.clone());
            svc
        })
        .try_build()
        .expect("a valid secret file must let the host build");

    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some(GOOD_SECRET),
        "the registration closure must see the secret resolved from Jwt.SecretFile"
    );
    assert_eq!(host.options().jwt.secret, GOOD_SECRET);
}

#[tokio::test]
async fn custom_issuer_resolving_jwt_secret_produces_tokens_the_host_verifies() {
    let dir = tempfile::tempdir().expect("a temporary directory is available");
    let path = write_secret_file(&dir, GOOD_SECRET.as_bytes());

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(configure_secret_file(path))
        .add_authentication()
        .register(|svc| {
            // The factory is invoked after the provider is built; `JwtSecret`
            // must already be registered by then (the host adds it before user
            // service configurations).
            svc.try_add_singleton::<dyn ITokenIssuer>(|resolver| {
                let secret = resolver
                    .resolve_arc::<JwtSecret>()
                    .expect("the host registers JwtSecret before user factories run");
                Arc::new(JwtTokenIssuer::from_secret(
                    secret.as_str(),
                    Duration::from_secs(3600),
                ))
            })
        })
        .try_build()
        .expect("a valid secret file must let the host build");

    let registered = host
        .provider()
        .get::<JwtSecret>()
        .expect("JwtSecret must be registered as a singleton");
    assert_eq!(registered.as_str(), GOOD_SECRET);

    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("the application issuer must win over the host default");
    let issued = issuer
        .issue(TokenClaims::new("user-1").with_claim("tenant_id", "acme"))
        .await
        .expect("issuing must succeed");

    // Verify with the very key the host's JwtAuth middleware was built from.
    let verifier: Arc<dyn IAuthenticationHandler> = Arc::new(JwtAuth::new(
        DecodingKey::from_secret(host.options().jwt.secret.as_bytes()),
        Validation::default(),
    ));
    let mut ctx = test_utils::TestHttpContext::new("GET", "/me")
        .with_header("authorization", &format!("Bearer {}", issued.token));
    let flow = jwt_middleware(verifier)
        .invoke(&mut ctx)
        .await
        .expect("a token signed with the resolved secret must authenticate");
    assert!(matches!(flow, std::ops::ControlFlow::Continue(())));
    assert_eq!(ctx.claims().map(|claims| claims.subject()), Some("user-1"));
}

/// Regression: the existing `register` signature and behaviour are unchanged.
#[derive(Debug)]
struct Marker;

#[test]
fn register_still_registers_services_without_options() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| svc.instance(Arc::new(Marker)))
        .try_build()
        .expect("host must build");

    assert!(host.provider().get::<Marker>().is_ok());
}

/// No configured secret means no `JwtSecret` value: resolving it is a start-up
/// error instead of handing a custom issuer an empty key.
#[test]
fn jwt_secret_is_absent_when_no_secret_is_configured() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.jwt.secret.clear();
                options.jwt.secret_file = None;
            });
        })
        .try_build()
        .expect("host must build");

    assert!(host.provider().get::<JwtSecret>().is_err());
}
