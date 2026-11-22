//! Declarative `#[validate]` checks: they run after binding, aggregate with
//! binding errors, and reject through the same `400 problem+json` shape.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

#[allow(clippy::ptr_arg)]
fn validate_code(value: &String) -> std::result::Result<(), String> {
    if value.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err("The `code` field must be alphanumeric.".to_string())
    }
}

#[derive(Debug, WebxRequestMeta)]
#[allow(dead_code)] // the fields exist to be validated, not read by the handler
struct CreateUserRequest {
    #[validate(length(min = 3, max = 8))]
    name: String,
    #[validate(email)]
    email: String,
    #[validate(range(min = 18, max = 120))]
    age: u8,
    #[validate(length(min = 1))]
    tags: Vec<String>,
    #[validate(custom = "validate_code")]
    code: Option<String>,
}

#[post("/api/users")]
impl IRequest<CreatedUser> for CreateUserRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct CreatedUser {
    name: String,
}

#[derive(Default)]
struct CreateUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CreateUserRequest, CreatedUser> for CreateUserHandler {
    async fn handle(&mut self, req: CreateUserRequest) -> ActionResult<CreatedUser> {
        Ok(CreatedUser { name: req.name })
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn spawn_host(port: u16) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return handle;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

async fn create(port: u16, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/users"))
        .json(&body)
        .send()
        .await
        .expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_values_are_a_400_with_field_errors() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let response = create(
        port,
        serde_json::json!({
            "name": "ab",
            "email": "not-an-email",
            "age": 12,
            "tags": [],
            "code": "bad code"
        }),
    )
    .await;
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.headers().get("content-type").expect("ct"),
        "application/problem+json"
    );
    let problem: serde_json::Value = response.json().await.expect("json");
    let errors = &problem["errors"];
    assert!(
        errors["name"][0]
            .as_str()
            .is_some_and(|m| m.contains("at least 3 characters")),
        "{problem}"
    );
    assert!(
        errors["email"][0]
            .as_str()
            .is_some_and(|m| m.contains("valid email")),
        "{problem}"
    );
    assert!(
        errors["age"][0]
            .as_str()
            .is_some_and(|m| m.contains("at least 18")),
        "{problem}"
    );
    assert!(
        errors["tags"][0]
            .as_str()
            .is_some_and(|m| m.contains("at least 1 item")),
        "{problem}"
    );
    assert!(
        errors["code"][0]
            .as_str()
            .is_some_and(|m| m.contains("alphanumeric")),
        "{problem}"
    );

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_values_pass_and_the_optional_rule_is_skipped() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let response = create(
        port,
        serde_json::json!({
            "name": "alice",
            "email": "alice@example.com",
            "age": 30,
            "tags": ["vip"]
        }),
    )
    .await;
    assert_eq!(response.status(), 200);
    let created: CreatedUser = response.json().await.expect("json");
    assert_eq!(created.name, "alice");

    handle.shutdown();
}
