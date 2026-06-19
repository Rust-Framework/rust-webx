mod test_utils;

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use rust_webapp_core::auth::IAuthenticationHandler;
use rust_webapp_host::auth_jwt::JwtAuth;
use serde::{Deserialize, Serialize};

fn now_plus_seconds(secs: u64) -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() + secs)
        .unwrap() as usize
}

fn now_minus_seconds(secs: u64) -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(secs))
        .unwrap() as usize
}

#[derive(Debug, Serialize, Deserialize)]
struct TestClaims {
    sub: String,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    exp: usize,
}

fn create_test_token(secret: &[u8], sub: &str, roles: &[&str], permissions: &[&str]) -> String {
    let claims = TestClaims {
        sub: sub.to_string(),
        roles: roles.iter().map(|s| s.to_string()).collect(),
        permissions: permissions.iter().map(|s| s.to_string()).collect(),
        exp: now_plus_seconds(3600),
    };
    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .unwrap()
}

fn create_expired_token(secret: &[u8]) -> String {
    let claims = TestClaims {
        sub: "expired-user".to_string(),
        roles: vec![],
        permissions: vec![],
        exp: now_minus_seconds(3600),
    };
    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .unwrap()
}

#[tokio::test]
async fn auth_valid_token_returns_claims() {
    let secret = b"test-secret-key-for-auth-testing";
    let token = create_test_token(secret, "user-123", &["user"], &["read"]);
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected")
        .with_header("authorization", &format!("Bearer {}", token));

    let result = auth.authenticate(&mut ctx).await;
    assert!(result.is_ok());
    let claims = result.unwrap();
    assert!(claims.is_some());
    let claims = claims.unwrap();
    assert_eq!(claims.subject(), "user-123");
    assert!(claims.roles().contains(&"user".to_string()));
    assert!(claims.permissions().contains(&"read".to_string()));
}

#[tokio::test]
async fn auth_no_authorization_header_returns_none() {
    let secret = b"test-secret-key";
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx = test_utils::TestHttpContext::new("GET", "/public");
    let result = auth.authenticate(&mut ctx).await.unwrap();
    assert!(result.is_none(), "No auth header should return None");
}

#[tokio::test]
async fn auth_empty_token_returns_none() {
    let secret = b"test-secret-key";
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx =
        test_utils::TestHttpContext::new("GET", "/").with_header("authorization", "Bearer ");

    let result = auth.authenticate(&mut ctx).await.unwrap();
    assert!(result.is_none(), "Empty Bearer token should return None");
}

#[tokio::test]
async fn auth_invalid_token_returns_error() {
    let secret = b"test-secret-key";
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("authorization", "Bearer invalid-token-data");

    let result = auth.authenticate(&mut ctx).await;
    assert!(result.is_err(), "Invalid token should return an error");
}

#[tokio::test]
async fn auth_wrong_secret_key_fails() {
    let secret = b"real-secret";
    let wrong_secret = b"wrong-secret";
    let token = create_test_token(secret, "user-1", &[], &[]);
    let auth = JwtAuth::new(
        DecodingKey::from_secret(wrong_secret),
        Validation::default(),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("authorization", &format!("Bearer {}", token));

    let result = auth.authenticate(&mut ctx).await;
    assert!(
        result.is_err(),
        "Token signed with different key should fail"
    );
}

#[tokio::test]
async fn auth_expired_token_fails() {
    let secret = b"test-secret-key-expiry";
    let token = create_expired_token(secret);
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("authorization", &format!("Bearer {}", token));

    let result = auth.authenticate(&mut ctx).await;
    assert!(result.is_err(), "Expired token should be rejected");
}

#[tokio::test]
async fn auth_bearer_prefix_case_sensitive() {
    let secret = b"test-secret-key";
    let token = create_test_token(secret, "user-1", &[], &[]);
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("authorization", &format!("bearer {}", token));

    let result = auth.authenticate(&mut ctx).await.unwrap();
    assert!(
        result.is_none(),
        "Lowercase 'bearer' should not be recognized"
    );
}

#[tokio::test]
async fn auth_token_with_extra_whitespace_handled() {
    let secret = b"test-secret-key";
    let token = create_test_token(secret, "user-1", &[], &[]);
    let auth = JwtAuth::new(DecodingKey::from_secret(secret), Validation::default());

    let mut ctx = test_utils::TestHttpContext::new("GET", "/")
        .with_header("authorization", &format!("Bearer  {}", token));

    let result = auth.authenticate(&mut ctx).await;
    assert!(
        result.is_ok(),
        "Token with extra whitespace after Bearer should work"
    );
    let claims = result.unwrap();
    assert!(claims.is_some());
}
