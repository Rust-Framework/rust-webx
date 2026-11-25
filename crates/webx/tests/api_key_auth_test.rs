//! End-to-end tests for the authentication scheme chain.
//!
//! Covers API keys over both accepted headers, invalid/expired keys, and
//! coexistence with the existing JWT scheme on the same host.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{EncodingKey, Header};
use serde::{Deserialize, Serialize};
use webx::*;

const JWT_SECRET: &str = "api-key-test-jwt-secret-at-least-32-bytes";

#[derive(Debug, Default, WebxRequestMeta)]
struct WhoAmIRequest {
    #[fromclaim]
    user: IdentityUser,
}

#[get("/whoami")]
impl IRequest<WhoAmI> for WhoAmIRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct WhoAmI {
    id: String,
    tenant: String,
    admin: bool,
}

#[derive(Default)]
struct WhoAmIHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<WhoAmIRequest, WhoAmI> for WhoAmIHandler {
    async fn handle(&mut self, req: WhoAmIRequest) -> ActionResult<WhoAmI> {
        let admin = req.user.is_in_role("admin");
        Ok(WhoAmI {
            id: req.user.id,
            tenant: req.user.tenant_id,
            admin,
        })
    }
}

/// Two active keys: one for the bearer header, one for `x-api-key`; anything
/// else is unknown.
struct TestKeys;

#[async_trait::async_trait]
impl IApiKeyValidator for TestKeys {
    async fn validate(&self, presented_key: &str) -> Result<Option<Box<dyn IClaims>>> {
        match presented_key {
            "sk-test-bearer" | "sk-test-x-api-key" => Ok(Some(Box::new(
                ApiKeyClaims::new("key-1")
                    .with_tenant("tenant-1")
                    .with_roles(["admin"]),
            ))),
            _ => Ok(None),
        }
    }
}

fn jwt_token() -> String {
    #[derive(Serialize)]
    struct Claims {
        sub: String,
        roles: Vec<String>,
        exp: usize,
    }

    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as usize
        + 3600;

    jsonwebtoken::encode(
        &Header::default(),
        &Claims {
            sub: "user-from-jwt".into(),
            roles: vec!["admin".into()],
            exp,
        },
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("token encodes")
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn spawn_host(with_jwt: bool) -> String {
    spawn_host_with(None, with_jwt).await
}

async fn spawn_host_with(failure_limit: Option<(u32, Duration)>, with_jwt: bool) -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");

    let mut scheme = ApiKeyScheme::new(Arc::new(TestKeys));
    if let Some((max_failures, window)) = failure_limit {
        scheme = scheme.with_failure_limit(max_failures, window);
    }
    let mut builder = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication_scheme(Arc::new(scheme));

    if with_jwt {
        builder = builder.use_authentication().configure(|app| {
            app.options(|options| {
                options.jwt.secret = JWT_SECRET.to_string();
            });
        });
    }

    let host = builder.build();
    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

#[tokio::test]
async fn api_key_in_bearer_header_authenticates() {
    let base = spawn_host(false).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/whoami"))
        .header("authorization", "Bearer sk-test-bearer")
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    let body: WhoAmI = response.json().await.expect("JSON body");
    assert_eq!(body.id, "key-1");
    assert_eq!(body.tenant, "tenant-1");
    assert!(body.admin);
}

#[tokio::test]
async fn api_key_in_x_api_key_header_authenticates() {
    let base = spawn_host(false).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/whoami"))
        .header("x-api-key", "sk-test-x-api-key")
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 200);
    let body: WhoAmI = response.json().await.expect("JSON body");
    assert_eq!(body.id, "key-1");
}

#[tokio::test]
async fn invalid_api_key_is_rejected_with_401() {
    let base = spawn_host(false).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/whoami"))
        .header("authorization", "Bearer sk-not-a-real-key")
        .send()
        .await
        .expect("request succeeds");

    assert_eq!(response.status(), 401);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert!(response.headers().get("www-authenticate").is_some());
    let body: serde_json::Value = response.json().await.expect("problem JSON");
    assert_eq!(body["status"], 401);
    assert!(
        !body.to_string().contains("sk-not-a-real-key"),
        "the rejected key must not be echoed"
    );
}

#[tokio::test]
async fn missing_credentials_are_401_for_a_claim_bound_endpoint() {
    let base = spawn_host(false).await;

    let response = reqwest::get(format!("{base}/whoami"))
        .await
        .expect("request succeeds");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn jwt_and_api_key_schemes_coexist() {
    let base = spawn_host(true).await;
    let client = reqwest::Client::new();

    let jwt = client
        .get(format!("{base}/whoami"))
        .header("authorization", format!("Bearer {}", jwt_token()))
        .send()
        .await
        .expect("JWT request succeeds");
    assert_eq!(jwt.status(), 200);
    let body: WhoAmI = jwt.json().await.expect("JSON body");
    assert_eq!(body.id, "user-from-jwt");
    assert_eq!(body.tenant, "");
    assert!(body.admin);

    let api_key = client
        .get(format!("{base}/whoami"))
        .header("authorization", "Bearer sk-test-bearer")
        .send()
        .await
        .expect("API-key request succeeds");
    assert_eq!(api_key.status(), 200);
    let body: WhoAmI = api_key.json().await.expect("JSON body");
    assert_eq!(body.id, "key-1");
    assert_eq!(body.tenant, "tenant-1");
}

#[tokio::test]
async fn repeated_invalid_keys_are_throttled_with_429() {
    let base = spawn_host_with(Some((2, Duration::from_secs(60))), false).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/whoami");

    for _ in 0..2 {
        let response = client
            .get(&url)
            .header("authorization", "Bearer sk-bruteforce-guess")
            .send()
            .await
            .expect("request succeeds");
        assert_eq!(response.status(), 401);
    }

    let limited = client
        .get(&url)
        .header("authorization", "Bearer sk-bruteforce-guess")
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(
        limited.status(),
        429,
        "a third guess from the same IP/prefix must be throttled"
    );
    assert!(limited.headers().get("retry-after").is_some());

    // A valid key uses a different prefix and still works.
    let valid = client
        .get(&url)
        .header("authorization", "Bearer sk-test-bearer")
        .send()
        .await
        .expect("request succeeds");
    assert_eq!(valid.status(), 200);
}

#[tokio::test]
async fn jwt_scheme_still_treats_invalid_tokens_as_anonymous() {
    let base = spawn_host(true).await;

    let response = reqwest::Client::new()
        .get(format!("{base}/whoami"))
        .header("authorization", "Bearer a.b.c")
        .send()
        .await
        .expect("request succeeds");

    // The JWT scheme matches the compact shape, rejects the signature, and the
    // endpoint's required identity binding produces 401 (not a 500).
    assert_eq!(response.status(), 401);
}
