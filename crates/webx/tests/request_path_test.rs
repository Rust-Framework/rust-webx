//! Integration tests covering the main request processing paths.
//!
//! Tests the full HTTP lifecycle: route matching → parameter binding →
//! handler dispatch → response serialization → status code mapping.

use std::net::TcpListener;

use rust_webx::*;
use serde::{Deserialize, Serialize};

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_host(port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|b| {
            b.useOptions(|o| {
                o.app.max_body_size = 100;
            });
        })
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------------------
// GET with path parameter
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct GetUserRequest {
    id: String,
}

#[derive(Serialize, Deserialize)]
struct UserResponse {
    id: String,
    name: String,
}

#[get("/users/{id}")]
impl IRequest<UserResponse> for GetUserRequest {}

#[derive(Default)]
struct GetUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<GetUserRequest, UserResponse> for GetUserHandler {
    async fn handle(&mut self, req: GetUserRequest) -> Result<UserResponse> {
        Ok(UserResponse {
            id: req.id.clone(),
            name: format!("User-{}", req.id),
        })
    }
}

#[tokio::test]
async fn get_with_path_param_returns_200() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/users/42", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "42");
    assert_eq!(body["name"], "User-42");
}

// ---------------------------------------------------------------------------
// POST with JSON body
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct CreateUserRequest {
    name: String,
    email: String,
}

#[derive(Serialize, Deserialize)]
struct CreateUserResponse {
    id: u32,
    name: String,
    email: String,
}

#[post("/users")]
impl IRequest<CreateUserResponse> for CreateUserRequest {}

#[derive(Default)]
struct CreateUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CreateUserRequest, CreateUserResponse> for CreateUserHandler {
    async fn handle(&mut self, req: CreateUserRequest) -> Result<CreateUserResponse> {
        Ok(CreateUserResponse {
            id: 1,
            name: req.name,
            email: req.email,
        })
    }
}

#[tokio::test]
async fn post_with_json_body_returns_200() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/users", port))
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "alice@example.com"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], 1);
    assert_eq!(body["name"], "Alice");
    assert_eq!(body["email"], "alice@example.com");
}

// ---------------------------------------------------------------------------
// POST with invalid JSON body → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_with_invalid_json_returns_400() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/users", port))
        .header("content-type", "application/json")
        .body("not valid json")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400);
}

// ---------------------------------------------------------------------------
// PUT with path param + body
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct UpdateUserRequest {
    #[serde(default)]
    id: String,
    name: String,
}

#[derive(Serialize, Deserialize)]
struct UpdateUserResponse {
    id: String,
    name: String,
    updated: bool,
}

#[put("/users/{id}")]
impl IRequest<UpdateUserResponse> for UpdateUserRequest {}

#[derive(Default)]
struct UpdateUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<UpdateUserRequest, UpdateUserResponse> for UpdateUserHandler {
    async fn handle(&mut self, req: UpdateUserRequest) -> Result<UpdateUserResponse> {
        Ok(UpdateUserResponse {
            id: req.id,
            name: req.name,
            updated: true,
        })
    }
}

#[tokio::test]
async fn put_with_path_and_body_returns_200() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::Client::new()
        .put(format!("http://127.0.0.1:{}/users/99", port))
        .json(&serde_json::json!({ "name": "Updated" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "99");
    assert_eq!(body["name"], "Updated");
    assert_eq!(body["updated"], true);
}

// ---------------------------------------------------------------------------
// DELETE → 204 No Content
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct DeleteUserRequest {
    id: String,
}

#[delete("/users/{id}")]
impl IRequest<()> for DeleteUserRequest {}

#[derive(Default)]
struct DeleteUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DeleteUserRequest, ()> for DeleteUserHandler {
    async fn handle(&mut self, _req: DeleteUserRequest) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn delete_returns_204_no_content() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::Client::new()
        .delete(format!("http://127.0.0.1:{}/users/5", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 204);
    assert!(resp.content_length().unwrap_or(0) == 0);
}

// ---------------------------------------------------------------------------
// 405 Method Not Allowed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_not_allowed_returns_405() {
    let port = find_free_port();
    spawn_host(port).await;

    // /users/{id} has GET, PUT, DELETE but not POST
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/users/1", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 405);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 405);
}

// ---------------------------------------------------------------------------
// 413 Payload Too Large
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oversized_body_returns_413() {
    let port = find_free_port();
    spawn_host(port).await;

    let big_body = "x".repeat(200); // max_body_size is 100

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/users", port))
        .header("content-type", "application/json")
        .body(big_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 413);
}

// ---------------------------------------------------------------------------
// Multiple path parameters
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct GetPostRequest {
    user_id: String,
    post_id: String,
}

#[derive(Serialize, Deserialize)]
struct PostResponse {
    user_id: String,
    post_id: String,
    title: String,
}

#[get("/users/{user_id}/posts/{post_id}")]
impl IRequest<PostResponse> for GetPostRequest {}

#[derive(Default)]
struct GetPostHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<GetPostRequest, PostResponse> for GetPostHandler {
    async fn handle(&mut self, req: GetPostRequest) -> Result<PostResponse> {
        Ok(PostResponse {
            user_id: req.user_id.clone(),
            post_id: req.post_id.clone(),
            title: format!("Post {} by user {}", req.post_id, req.user_id),
        })
    }
}

#[tokio::test]
async fn multiple_path_params_parsed_correctly() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!(
        "http://127.0.0.1:{}/users/10/posts/20",
        port
    ))
    .await
    .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["user_id"], "10");
    assert_eq!(body["post_id"], "20");
    assert_eq!(body["title"], "Post 20 by user 10");
}

// ---------------------------------------------------------------------------
// x-request-id propagation (upstream header echoed back)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upstream_request_id_is_propagated() {
    let port = find_free_port();
    spawn_host(port).await;

    let custom_id = "my-trace-id-12345";
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/health", port))
        .header("x-request-id", custom_id)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let echoed = resp
        .headers()
        .get("x-request-id")
        .expect("x-request-id should be present");
    assert_eq!(echoed.to_str().unwrap(), custom_id);
}

#[tokio::test]
async fn missing_request_id_generates_one() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let request_id = resp
        .headers()
        .get("x-request-id")
        .expect("x-request-id should be generated");
    assert!(!request_id.is_empty());
}

// ---------------------------------------------------------------------------
// GET with query parameters
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct SearchRequest {
    q: String,
    page: String,
}

#[derive(Serialize, Deserialize)]
struct SearchResponse {
    q: String,
    page: String,
}

#[get("/search")]
impl IRequest<SearchResponse> for SearchRequest {}

#[derive(Default)]
struct SearchHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<SearchRequest, SearchResponse> for SearchHandler {
    async fn handle(&mut self, req: SearchRequest) -> Result<SearchResponse> {
        Ok(SearchResponse {
            q: req.q,
            page: req.page,
        })
    }
}

#[tokio::test]
async fn get_with_query_params_returns_200() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!(
        "http://127.0.0.1:{}/search?q=hello&page=2",
        port
    ))
    .await
    .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["q"], "hello");
    assert_eq!(body["page"], "2");
}

// ---------------------------------------------------------------------------
// Error status code mapping
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct FailRequest {
    code: String,
}

#[derive(Serialize, Deserialize)]
struct FailResponse {
    message: String,
}

#[get("/fail/{code}")]
impl IRequest<FailResponse> for FailRequest {}

#[derive(Default)]
struct FailHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<FailRequest, FailResponse> for FailHandler {
    async fn handle(&mut self, req: FailRequest) -> Result<FailResponse> {
        match req.code.as_str() {
            "401" => Err(Error::Unauthorized("custom unauthorized".into())),
            "403" => Err(Error::Forbidden("custom forbidden".into())),
            "422" => Err(Error::Status(422, "validation failed".into())),
            "500" => Err(Error::Internal("internal error".into())),
            _ => Ok(FailResponse {
                message: "ok".into(),
            }),
        }
    }
}

#[tokio::test]
async fn error_unauthorized_maps_to_401() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/fail/401", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);
    assert!(body["detail"].as_str().unwrap().contains("custom unauthorized"));
}

#[tokio::test]
async fn error_forbidden_maps_to_403() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/fail/403", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 403);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 403);
}

#[tokio::test]
async fn error_status_custom_maps_correctly() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/fail/422", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 422);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 422);
}

#[tokio::test]
async fn error_internal_maps_to_500() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/fail/500", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 500);
}

// ---------------------------------------------------------------------------
// 404 for unknown route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_route_returns_404_problem_json() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/totally/unknown", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 404);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("application/problem+json"));
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "Not Found");
}

// ---------------------------------------------------------------------------
// Content-Type header on JSON responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_response_has_correct_content_type() {
    let port = find_free_port();
    spawn_host(port).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/users/1", port))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("application/json"));
}
