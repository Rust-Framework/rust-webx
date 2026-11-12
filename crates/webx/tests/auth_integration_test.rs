//! End-to-end integration tests for JWT authentication + #[authorize] macro.
//!
//! Verifies the full HTTP path: JwtAuth middleware → RouteDispatch 401/403 checks.

use std::net::TcpListener;

use jsonwebtoken::{EncodingKey, Header};
use serde::{Deserialize, Serialize};
use webx::*;

const TEST_SECRET: &[u8] = b"test-secret-key-for-integration";

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

fn create_test_token(secret: &[u8], sub: &str, roles: &[&str]) -> String {
    let claims = TestClaims {
        sub: sub.to_string(),
        roles: roles.iter().map(|s| s.to_string()).collect(),
        permissions: Vec::new(),
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
        permissions: Vec::new(),
        exp: now_minus_seconds(3600),
    };
    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .unwrap()
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct ProtectedRequest;

#[get("/protected")]
#[authorize(role = "admin")]
impl IRequest<String> for ProtectedRequest {}

#[derive(Default)]
struct ProtectedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<ProtectedRequest, String> for ProtectedHandler {
    async fn handle(&mut self, _: ProtectedRequest) -> ActionResult<String> {
        Ok("admin-area".to_string())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct MeRequest;

#[get("/me")]
#[authorize]
impl IRequest<String> for MeRequest {}

#[derive(Default)]
struct MeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<MeRequest, String> for MeHandler {
    async fn handle(&mut self, _: MeRequest) -> ActionResult<String> {
        Ok("authenticated".to_string())
    }
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_auth_host(port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let builder = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication()
        .configure(|b| {
            b.useOptions(|o| {
                o.jwt.secret = String::from_utf8_lossy(TEST_SECRET).to_string();
            });
        });
    let host = builder.build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

#[tokio::test]
async fn auth_no_token_returns_401() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/protected", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);
    assert_eq!(body["detail"], "authentication required");
    assert_eq!(body["title"], "Unauthorized");
}

#[tokio::test]
async fn auth_valid_admin_token_returns_200() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let token = create_test_token(TEST_SECRET, "admin-1", &["admin"]);
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/protected", port))
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, "admin-area");
}

#[tokio::test]
async fn auth_wrong_role_returns_403() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let token = create_test_token(TEST_SECRET, "user-1", &["user"]);
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/protected", port))
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 403);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 403);
    assert_eq!(body["required_roles"], serde_json::json!(["admin"]));
}

/// #18: an expired token does not fail authentication — it degrades the request
/// to anonymous. The protected route's `#[authorize(role = "admin")]` then sees
/// no principal and the single unauthorized writer answers 401 with the
/// challenge (detail "authentication required", no decoder internals).
#[tokio::test]
async fn auth_expired_token_returns_401() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let token = create_expired_token(TEST_SECRET);
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/protected", port))
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);
    let challenge = resp
        .headers()
        .get("www-authenticate")
        .expect("401 must carry a WWW-Authenticate challenge")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        challenge.to_ascii_lowercase().contains("bearer"),
        "got {challenge}"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);
    assert_eq!(body["title"], "Unauthorized");
    assert_eq!(
        body["detail"], "authentication required",
        "an expired token is anonymous, not a token-validation error"
    );
}

#[tokio::test]
async fn auth_authenticated_only_returns_200() {
    let port = find_free_port();
    spawn_auth_host(port).await;

    let token = create_test_token(TEST_SECRET, "user-1", &["user"]);
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/me", port))
        .header("authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, "authenticated");
}
