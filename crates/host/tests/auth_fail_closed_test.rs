//! 2.5 M1 — authentication fails closed when the revocation backend is down.

mod test_utils;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::{DecodingKey, Validation};
use webx_core::auth::IAuthenticationScheme;
use webx_core::error::{Error, Result};
use webx_core::security::{ISessionStore, ITokenIssuer, TokenClaims};
use webx_host::auth::JwtScheme;
use webx_host::auth_jwt::{JwtAuth, JwtTokenIssuer};
use webx_host::session_cache::CachedSessionStore;

const SECRET: &str = "fail-closed-test-secret-at-least-32-bytes";

struct FailingStore;

#[async_trait]
impl ISessionStore for FailingStore {
    async fn revoke(&self, _jti: &str) -> Result<()> {
        Err(Error::Internal("store down".to_string()))
    }

    async fn is_revoked(&self, _jti: &str) -> Result<bool> {
        Err(Error::Internal("store down".to_string()))
    }
}

#[tokio::test]
async fn jwt_scheme_propagates_a_store_failure_instead_of_authenticating() {
    let issuer = JwtTokenIssuer::from_secret(SECRET.as_bytes(), Duration::from_secs(3600));
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue token");

    let auth = JwtAuth::new(
        DecodingKey::from_secret(SECRET.as_bytes()),
        Validation::default(),
    );
    let scheme = JwtScheme::new(Arc::new(auth), Some(Arc::new(FailingStore)));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected")
        .with_header("authorization", &format!("Bearer {}", issued.token));
    let error = scheme
        .authenticate(&mut ctx)
        .await
        .err()
        .expect("a store failure must fail the request");

    assert!(matches!(error, Error::Internal(_)), "got {error:?}");
}

#[tokio::test]
async fn cached_store_does_not_cache_backend_errors() {
    let store = CachedSessionStore::new(Arc::new(FailingStore), Duration::from_secs(60));
    let error = store
        .is_revoked("jti-a")
        .await
        .expect_err("backend error must propagate");
    assert!(matches!(error, Error::Internal(_)), "got {error:?}");
}
