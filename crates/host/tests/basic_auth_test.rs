//! 2.5 M1 閳?HTTP Basic scheme (RFC 7617).

mod test_utils;

use std::sync::Arc;

use base64::Engine as _;
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_host::auth_basic::{BasicClaims, BasicScheme, IBasicCredentialValidator};

struct StaticValidator {
    user: &'static str,
    password: &'static str,
}

#[async_trait::async_trait]
impl IBasicCredentialValidator for StaticValidator {
    async fn validate(&self, username: &str, password: &str) -> Result<Option<Box<dyn IClaims>>> {
        Ok((username == self.user && password == self.password)
            .then(|| Box::new(BasicClaims::new(username)) as Box<dyn IClaims>))
    }
}

fn scheme() -> BasicScheme {
    BasicScheme::new(Arc::new(StaticValidator {
        user: "alice",
        password: "s3cret:with:colons",
    }))
}

fn encode(user: &str, password: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
}

fn request(header: String) -> test_utils::TestHttpContext {
    test_utils::TestHttpContext::new("GET", "/").with_header("authorization", &header)
}

#[tokio::test]
async fn valid_credentials_produce_claims() {
    let scheme = scheme();
    let mut ctx = request(format!("Basic {}", encode("alice", "s3cret:with:colons")));
    let claims = scheme
        .authenticate(&mut ctx)
        .await
        .expect("authenticate")
        .expect("valid credentials");
    assert_eq!(claims.subject(), "alice");
}

#[tokio::test]
async fn wrong_password_is_unauthorized() {
    let scheme = scheme();
    let mut ctx = request(format!("Basic {}", encode("alice", "wrong")));
    let error = scheme
        .authenticate(&mut ctx)
        .await
        .err()
        .expect("must reject");
    assert!(matches!(error, Error::Unauthorized(_)), "got {error:?}");
}

#[tokio::test]
async fn malformed_base64_is_unauthorized() {
    let scheme = scheme();
    let mut ctx = request("Basic not-base64!".to_string());
    let error = scheme
        .authenticate(&mut ctx)
        .await
        .err()
        .expect("must reject");
    assert!(matches!(error, Error::Unauthorized(_)), "got {error:?}");
}

#[tokio::test]
async fn missing_colon_or_empty_user_is_unauthorized() {
    let scheme = scheme();
    let no_colon = base64::engine::general_purpose::STANDARD.encode("alice");
    let mut ctx = request(format!("Basic {no_colon}"));
    assert!(scheme.authenticate(&mut ctx).await.is_err());

    let empty_user = base64::engine::general_purpose::STANDARD.encode(":password");
    let mut ctx = request(format!("Basic {empty_user}"));
    assert!(scheme.authenticate(&mut ctx).await.is_err());
}

#[tokio::test]
async fn scheme_matches_only_basic() {
    let scheme = scheme();
    assert!(scheme.matches(&request(format!("Basic {}", encode("alice", "x")))));
    assert!(scheme.matches(&request(format!("basic {}", encode("alice", "x")))));
    assert!(!scheme.matches(&request("Bearer a.b.c".to_string())));
    assert!(!scheme.matches(&test_utils::TestHttpContext::new("GET", "/")));
}
