//! WEBX-2 end-to-end: a revoked-but-signature-valid token must be answered by
//! the framework's single 401 writer (problem+json + `WWW-Authenticate`) before
//! the endpoint runs, not accepted until it expires.

use std::sync::Arc;
use std::time::Duration;

use webx::*;

const TEST_SECRET: &str = "test-secret-key-for-revocation-e2e-0123456789";

#[derive(Default, serde::Serialize, serde::Deserialize, WebxRequestMeta)]
struct SessionProbeRequest;

#[get("/session/protected")]
#[authorize]
impl IRequest<String> for SessionProbeRequest {}

#[derive(Default)]
struct SessionProbeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<SessionProbeRequest, String> for SessionProbeHandler {
    async fn handle(&mut self, _: SessionProbeRequest) -> ActionResult<String> {
        Ok("session-ok".to_string())
    }
}

fn test_secret() -> &'static str {
    TEST_SECRET
}

fn issuer() -> JwtTokenIssuer {
    JwtTokenIssuer::from_secret(test_secret(), Duration::from_secs(3600))
}

fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Spawn a host whose authentication middleware consults `store` for revocations.
async fn spawn_host(port: u16, store: Arc<InMemorySessionStore>) {
    let store: Arc<dyn ISessionStore> = store;
    let builder = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication()
        .use_session_store(store)
        .configure(|b| {
            b.useOptions(|o| {
                o.jwt.secret = test_secret().to_string();
            });
        });
    let host = builder.build();
    tokio::spawn(async move { host.run_at(&format!("127.0.0.1:{port}")).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

#[tokio::test]
async fn revoked_token_is_rejected_before_the_endpoint_with_the_framework_401() {
    let port = find_free_port();
    let store = Arc::new(InMemorySessionStore::new());

    let issued = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issuing must succeed");
    store
        .revoke(&issued.jti)
        .await
        .expect("logout must revoke the token");

    spawn_host(port, Arc::clone(&store)).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/session/protected"))
        .header("authorization", format!("Bearer {}", issued.token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);
    let challenge = resp
        .headers()
        .get("www-authenticate")
        .expect("a 401 must carry a WWW-Authenticate challenge")
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
    assert_eq!(body["detail"], "token has been revoked");
}

#[tokio::test]
async fn unrevoked_token_still_reaches_the_endpoint() {
    let port = find_free_port();
    let store = Arc::new(InMemorySessionStore::new());

    let issued = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issuing must succeed");
    spawn_host(port, Arc::clone(&store)).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/session/protected"))
        .header("authorization", format!("Bearer {}", issued.token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, "session-ok");
}
