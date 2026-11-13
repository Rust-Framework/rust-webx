//! WEBX-2: token issuance and session-revocation ports.
//!
//! Covers the attack path that used to be impossible to close at the framework
//! level: a token that is still signature-valid but has been revoked must be
//! rejected by the authentication middleware, not accepted until it expires.

mod test_utils;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use serde_json::json;
use webx_core::error::Error;
use webx_core::http::IClaimsExt;
use webx_core::middleware::IMiddleware;
use webx_core::security::{
    InMemorySessionStore, ISessionStore, ITokenIssuer, IssuedToken, TokenClaims,
};
use webx_host::auth_jwt::{
    jwt_middleware, jwt_middleware_with_revocation, JwtAuth, JwtTokenIssuer,
};

const SECRET: &str = "test-secret-key-for-session-revocation-0123456789";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn verifier() -> Arc<dyn webx_core::auth::IAuthenticationHandler> {
    Arc::new(JwtAuth::new(
        DecodingKey::from_secret(SECRET.as_bytes()),
        Validation::default(),
    ))
}

fn issuer() -> JwtTokenIssuer {
    JwtTokenIssuer::from_secret(SECRET, Duration::from_secs(3600))
}

async fn issue(claims: TokenClaims) -> IssuedToken {
    issuer().issue(claims).await.expect("issuing must succeed")
}

fn bearer(token: &str) -> test_utils::TestHttpContext {
    test_utils::TestHttpContext::new("GET", "/me")
        .with_header("authorization", &format!("Bearer {token}"))
}

#[tokio::test]
async fn issued_token_carries_subject_roles_permissions_jti_and_expiry() {
    let before = now_secs();
    let issued = issue(
        TokenClaims::new("user-1")
            .with_roles(["admin"])
            .with_permissions(["order.create"])
            .with_claim("tenant_id", "acme"),
    )
    .await;

    assert!(!issued.jti.is_empty(), "a token must carry a jti");
    assert!(
        issued.expires_at_unix_secs > before,
        "expires_at_unix_secs must be in the future, got {} (now {before})",
        issued.expires_at_unix_secs
    );

    let decoded = jsonwebtoken::decode::<serde_json::Value>(
        &issued.token,
        &DecodingKey::from_secret(SECRET.as_bytes()),
        &Validation::default(),
    )
    .expect("the issued token must verify with the same secret");
    let claims = decoded.claims;

    assert_eq!(claims["sub"], "user-1");
    assert_eq!(claims["jti"], issued.jti);
    assert_eq!(claims["roles"], json!(["admin"]));
    assert_eq!(claims["permissions"], json!(["order.create"]));
    assert_eq!(claims["tenant_id"], "acme");
    assert_eq!(claims["exp"].as_u64(), Some(issued.expires_at_unix_secs));
}

#[tokio::test]
async fn issuer_rejects_an_empty_subject_with_a_typed_error() {
    let err = issuer()
        .issue(TokenClaims::new("   "))
        .await
        .expect_err("an empty subject is a caller bug, not a token");
    assert!(matches!(err, Error::Validation(_)), "got {err:?}");
    assert_eq!(err.status_code(), 400);
}

#[tokio::test]
async fn issued_token_round_trips_through_the_auth_middleware() {
    let issued = issue(TokenClaims::new("user-1").with_claim("tenant_id", "acme")).await;

    let mut ctx = bearer(&issued.token);
    let flow = jwt_middleware(verifier())
        .invoke(&mut ctx)
        .await
        .expect("a valid token must authenticate");
    assert!(matches!(flow, std::ops::ControlFlow::Continue(())));

    let claims = ctx.claims().expect("claims must be stored");
    assert_eq!(claims.subject(), "user-1");
    assert_eq!(claims.jti(), Some(issued.jti.as_str()));
    assert_eq!(claims.get_tenantid(), Some("acme"));
}

#[tokio::test]
async fn revoked_token_is_unauthorized_at_the_middleware() {
    let store = Arc::new(InMemorySessionStore::new());
    let issued = issue(TokenClaims::new("user-1")).await;

    // Logout: revoke the very token that was just issued.
    store.revoke(&issued.jti).await.expect("revoke must succeed");
    assert!(store.is_revoked(&issued.jti).await.unwrap());

    let mut ctx = bearer(&issued.token);
    let err = match jwt_middleware_with_revocation(
        verifier(),
        Arc::clone(&store) as Arc<dyn ISessionStore>,
    )
    .invoke(&mut ctx)
    .await
    {
        Ok(_) => panic!("a revoked token must be rejected"),
        Err(err) => err,
    };

    assert!(matches!(err, Error::Unauthorized(_)), "got {err:?}");
    assert_eq!(err.status_code(), 401);
    assert!(
        ctx.claims().is_none(),
        "revoked credentials must not populate the principal"
    );
}

#[tokio::test]
async fn unrevoked_token_passes_the_revocation_check() {
    let store = Arc::new(InMemorySessionStore::new());
    let issued = issue(TokenClaims::new("user-1")).await;

    let mut ctx = bearer(&issued.token);
    let flow = jwt_middleware_with_revocation(
        verifier(),
        Arc::clone(&store) as Arc<dyn ISessionStore>,
    )
    .invoke(&mut ctx)
    .await
    .expect("an unrevoked token must authenticate");
    assert!(matches!(flow, std::ops::ControlFlow::Continue(())));
    assert_eq!(ctx.claims().map(|c| c.subject()), Some("user-1"));
}

/// A token with no `jti` cannot match a revocation record: the middleware must
/// leave the 1.1 behaviour untouched instead of rejecting it or panicking.
#[tokio::test]
async fn token_without_jti_is_not_treated_as_revoked() {
    let store = Arc::new(InMemorySessionStore::new());
    let token = jsonwebtoken::encode(
        &Header::default(),
        &json!({ "sub": "user-1", "exp": now_secs() + 3600 }),
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("encoding a jti-less token must succeed");

    let mut ctx = bearer(&token);
    let flow = jwt_middleware_with_revocation(verifier(), store as Arc<dyn ISessionStore>)
        .invoke(&mut ctx)
        .await
        .expect("missing jti must not be an authentication failure");
    assert!(matches!(flow, std::ops::ControlFlow::Continue(())));
    assert_eq!(ctx.claims().map(|c| c.subject()), Some("user-1"));
}

/// Without a registered store the middleware keeps the 1.1 behaviour: verify
/// only, no revocation lookup.
#[tokio::test]
async fn no_session_store_means_no_revocation_lookup() {
    let issued = issue(TokenClaims::new("user-1")).await;

    let mut ctx = bearer(&issued.token);
    let flow = jwt_middleware(verifier())
        .invoke(&mut ctx)
        .await
        .expect("plain jwt_middleware must still authenticate");
    assert!(matches!(flow, std::ops::ControlFlow::Continue(())));
    assert!(ctx.claims().is_some());
}

#[tokio::test]
async fn revoke_all_for_subject_is_unsupported_by_default_instead_of_silently_ok() {
    let store = InMemorySessionStore::new();
    let err = store
        .revoke_all_for_subject("user-1")
        .await
        .expect_err("the in-memory store cannot enumerate a subject's tokens");
    assert!(matches!(err, Error::State(_)), "got {err:?}");
}

/// Build a host that has authentication enabled and `Jwt.Secret` set, so the
/// default `ITokenIssuer` is registered.
fn host_with_jwt(expiration_minutes: Option<u64>) -> webx_host::Host {
    webx_host::Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .add_authentication()
        .configure(move |b| {
            b.useOptions(move |o| {
                o.jwt.secret = SECRET.to_string();
                if let Some(minutes) = expiration_minutes {
                    o.jwt.expiration_minutes = minutes;
                }
            });
        })
        .try_build()
        .expect("host must build")
}

/// `Jwt.ExpirationMinutes` (appsettings / `APP__Jwt__ExpirationMinutes`) drives
/// the lifetime of tokens minted by the framework's default issuer.
#[tokio::test]
async fn jwt_expiration_minutes_config_drives_the_default_issuer() {
    let host = host_with_jwt(Some(5));
    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("add_authentication must register the default issuer");
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issuing must succeed");

    let now = now_secs();
    let lower = now + 5 * 60 - 5;
    let upper = now + 5 * 60 + 5;
    assert!(
        (lower..=upper).contains(&issued.expires_at_unix_secs),
        "expires_at_unix_secs {} must be ~5 minutes from {now}",
        issued.expires_at_unix_secs
    );
}

#[tokio::test]
async fn jwt_expiration_defaults_to_60_minutes() {
    let host = host_with_jwt(None);
    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("add_authentication must register the default issuer");
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issuing must succeed");

    let now = now_secs();
    let expires_in = issued.expires_at_unix_secs.saturating_sub(now);
    assert!(
        (59 * 60..=61 * 60).contains(&expires_in),
        "default lifetime must be ~60 minutes, got {expires_in}s"
    );
}
