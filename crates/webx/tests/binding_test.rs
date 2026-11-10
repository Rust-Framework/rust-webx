//! End-to-end tests for the request binding layer.
//!
//! Covers the sources that used to be unreachable or silently dropped:
//! non-string query values, repeated query keys, headers, claims, and mixed
//! path + body + header models — plus the 400 shape that reports every field
//! failure at once.

use std::net::TcpListener;

use jsonwebtoken::{EncodingKey, Header};
use serde::{Deserialize, Serialize};
use webx::*;

const TEST_SECRET: &[u8] = b"binding-test-secret";

// ---------------------------------------------------------------------------
// Query binding
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct QueryRequest {
    q: String,
    page: u32,
    size: Option<u32>,
    ids: Vec<u32>,
}

#[get("/bind/query")]
impl IRequest<serde_json::Value> for QueryRequest {}

#[derive(Default)]
struct QueryHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<QueryRequest, serde_json::Value> for QueryHandler {
    async fn handle(&mut self, req: QueryRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "q": req.q,
            "page": req.page,
            "size": req.size,
            "ids": req.ids,
        }))
    }
}

// ---------------------------------------------------------------------------
// Header binding
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct HeaderRequest {
    #[frompath]
    id: String,
    #[fromheader("X-Tenant")]
    tenant: String,
    #[fromheader]
    x_trace_id: Option<String>,
    /// A repeated header binds as a list, comma-separated.
    #[fromheader("X-Scopes")]
    scopes: Vec<String>,
}

#[get("/bind/header/{id}")]
impl IRequest<serde_json::Value> for HeaderRequest {}

#[derive(Default)]
struct HeaderHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<HeaderRequest, serde_json::Value> for HeaderHandler {
    async fn handle(&mut self, req: HeaderRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "tenant": req.tenant,
            "trace": req.x_trace_id,
            "scopes": req.scopes,
        }))
    }
}

// ---------------------------------------------------------------------------
// Claim binding
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct ClaimRequest {
    #[fromclaim("sub")]
    user: String,
    #[fromclaim("roles")]
    roles: Vec<String>,
    #[fromclaim("tenant_id")]
    tenant: Option<String>,
}

#[get("/bind/claim")]
impl IRequest<serde_json::Value> for ClaimRequest {}

#[derive(Default)]
struct ClaimHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<ClaimRequest, serde_json::Value> for ClaimHandler {
    async fn handle(&mut self, req: ClaimRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "user": req.user,
            "roles": req.roles,
            "tenant": req.tenant,
        }))
    }
}

// ---------------------------------------------------------------------------
// Standard identity (`CurrentUser`)
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct CurrentUserRequest {
    #[fromclaim]
    user: CurrentUser,
}

#[get("/bind/user")]
impl IRequest<serde_json::Value> for CurrentUserRequest {}

#[derive(Default)]
struct CurrentUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CurrentUserRequest, serde_json::Value> for CurrentUserHandler {
    async fn handle(&mut self, req: CurrentUserRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.user.id,
            "name": req.user.name,
            "email": req.user.email,
            "tenant": req.user.tenant_id,
            "roles": req.user.roles,
            "user_id": req.user_id(),
            "user_name": req.user_name(),
            "is_admin": req.is_in_role("admin"),
            "requires_ok": req.require_user().is_ok(),
        }))
    }
}

#[derive(Default, WebxRequestMeta)]
struct OptionalCurrentUserRequest {
    #[fromclaim]
    user: Option<CurrentUser>,
}

#[get("/bind/user-optional")]
impl IRequest<serde_json::Value> for OptionalCurrentUserRequest {}

#[derive(Default)]
struct OptionalCurrentUserHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<OptionalCurrentUserRequest, serde_json::Value> for OptionalCurrentUserHandler {
    async fn handle(&mut self, req: OptionalCurrentUserRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "user_id": req.user_id(),
            "anonymous": req.current_user().is_none(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Mixed path + body + header
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct MixedRequest {
    #[frompath]
    id: String,
    name: String,
    #[fromheader("X-Actor")]
    actor: String,
}

#[put("/bind/mixed/{id}")]
impl IRequest<serde_json::Value> for MixedRequest {}

#[derive(Default)]
struct MixedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<MixedRequest, serde_json::Value> for MixedHandler {
    async fn handle(&mut self, req: MixedRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "name": req.name,
            "actor": req.actor,
        }))
    }
}

// ---------------------------------------------------------------------------
// `#[request(queryall)]`
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
#[request(queryall)]
struct QueryAllRequest {
    term: String,
    limit: u32,
}

#[post("/bind/query-all")]
impl IRequest<serde_json::Value> for QueryAllRequest {}

#[derive(Default)]
struct QueryAllHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<QueryAllRequest, serde_json::Value> for QueryAllHandler {
    async fn handle(&mut self, req: QueryAllRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({ "term": req.term, "limit": req.limit }))
    }
}

// ---------------------------------------------------------------------------
// Method-dependent priority for unmarked fields
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct PriorityGetRequest {
    key: String,
}

#[get("/bind/priority/{key}")]
impl IRequest<serde_json::Value> for PriorityGetRequest {}

#[derive(Default)]
struct PriorityGetHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PriorityGetRequest, serde_json::Value> for PriorityGetHandler {
    async fn handle(&mut self, req: PriorityGetRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({ "key": req.key }))
    }
}

#[derive(Default, Deserialize, WebxRequestMeta)]
struct PriorityWriteRequest {
    #[frompath]
    id: String,
    key: String,
}

#[post("/bind/priority-body/{id}")]
impl IRequest<serde_json::Value> for PriorityWriteRequest {}

#[derive(Default)]
struct PriorityWriteHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PriorityWriteRequest, serde_json::Value> for PriorityWriteHandler {
    async fn handle(&mut self, req: PriorityWriteRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({ "id": req.id, "key": req.key }))
    }
}

// ---------------------------------------------------------------------------
// Composite (complex) body fields
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize)]
struct Address {
    city: String,
    zip: String,
}

#[derive(Default, Deserialize)]
struct LineItem {
    sku: String,
    qty: u32,
}

#[derive(Default, Deserialize)]
struct Meta {
    source: String,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
struct ComplexRequest {
    #[frompath]
    id: String,
    address: Address,
    items: Vec<LineItem>,
    meta: Option<Meta>,
    note: Option<String>,
}

#[put("/bind/complex/{id}")]
impl IRequest<serde_json::Value> for ComplexRequest {}

#[derive(Default)]
struct ComplexHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<ComplexRequest, serde_json::Value> for ComplexHandler {
    async fn handle(&mut self, req: ComplexRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "city": req.address.city,
            "zip": req.address.zip,
            "items": req.items.iter().map(|i| i.sku.clone()).collect::<Vec<_>>(),
            "qty_total": req.items.iter().map(|i| i.qty).sum::<u32>(),
            "meta": req.meta.map(|m| m.source),
            "note": req.note,
        }))
    }
}

// ---------------------------------------------------------------------------
// Explicit custom scalars (`FromStr` types)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Slug(String);

impl std::str::FromStr for Slug {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Slug(value.to_ascii_lowercase()))
    }
}

/// No `Deserialize`/`Default`: a request model does not need either when every
/// field has an explicit scalar source.
#[derive(WebxRequestMeta)]
struct CustomScalarRequest {
    #[frompath]
    id: Slug,
    #[fromquery]
    filter: Slug,
}

#[get("/bind/custom/{id}")]
impl IRequest<serde_json::Value> for CustomScalarRequest {}

#[derive(Default)]
struct CustomScalarHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CustomScalarRequest, serde_json::Value> for CustomScalarHandler {
    async fn handle(&mut self, req: CustomScalarRequest) -> Result<serde_json::Value> {
        Ok(serde_json::json!({ "id": req.id.0, "filter": req.filter.0 }))
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct TokenClaims {
    sub: String,
    name: String,
    email: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    exp: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant_id: Option<String>,
}

fn token(tenant: Option<&str>) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize
        + 3600;
    let claims = TokenClaims {
        sub: "user-42".to_string(),
        name: "Ada Lovelace".to_string(),
        email: "ada@example.com".to_string(),
        roles: vec!["admin".to_string(), "auditor".to_string()],
        permissions: Vec::new(),
        exp,
        tenant_id: tenant.map(str::to_string),
    };
    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .unwrap()
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_host() -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication()
        .configure(|b| {
            b.useOptions(|o| {
                o.jwt.secret = String::from_utf8_lossy(TEST_SECRET).to_string();
            });
        })
        .build();
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        host.run_at(&addr).await.unwrap();
    });
    // Wait for the listener to accept connections.
    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_binds_non_string_types_and_repeated_keys() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::get(format!(
        "{base}/bind/query?q=hello&page=2&size=10&ids=1&ids=2&ids=3"
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(body["q"], "hello");
    assert_eq!(body["page"], 2);
    assert_eq!(body["size"], 10);
    assert_eq!(body["ids"], serde_json::json!([1, 2, 3]));
}

#[tokio::test]
async fn query_expands_comma_separated_lists() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::get(format!("{base}/bind/query?q=x&page=1&ids=4,5,6"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["ids"], serde_json::json!([4, 5, 6]));
    assert_eq!(body["size"], serde_json::Value::Null);
}

#[tokio::test]
async fn invalid_query_value_reports_the_field() {
    let base = spawn_host().await;
    let resp = reqwest::get(format!("{base}/bind/query?q=x&page=abc"))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400);
    let problem: serde_json::Value = resp.json().await.unwrap();
    let page = &problem["errors"]["page"];
    assert!(
        page.as_array().is_some_and(|messages| !messages.is_empty()),
        "expected a `page` error, got {problem}"
    );
}

#[tokio::test]
async fn query_all_binds_unmarked_fields_on_a_post() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/bind/query-all?term=abc&limit=5"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["term"], "abc");
    assert_eq!(body["limit"], 5);
}

#[tokio::test]
async fn missing_required_query_field_reports_the_field() {
    let base = spawn_host().await;
    let resp = reqwest::get(format!("{base}/bind/query?q=x"))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400);
    let problem: serde_json::Value = resp.json().await.unwrap();
    assert!(
        problem["errors"]["page"].is_array(),
        "expected a `page` error, got {problem}"
    );
}

#[tokio::test]
async fn headers_bind_case_insensitively() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/bind/header/7"))
        .header("x-tenant", "acme")
        .header("X-Trace-Id", "trace-1")
        .header("X-Scopes", "read, write")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["id"], "7");
    assert_eq!(body["tenant"], "acme");
    assert_eq!(body["trace"], "trace-1");
    assert_eq!(body["scopes"], serde_json::json!(["read", "write"]));
}

#[tokio::test]
async fn missing_required_header_reports_the_field() {
    let base = spawn_host().await;
    let resp = reqwest::Client::new()
        .get(format!("{base}/bind/header/7"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400);
    let problem: serde_json::Value = resp.json().await.unwrap();
    assert!(
        problem["errors"]["tenant"].is_array(),
        "expected a `tenant` error, got {problem}"
    );
}

#[tokio::test]
async fn claims_bind_subject_and_roles() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/bind/claim"))
        .bearer_auth(token(Some("acme")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["user"], "user-42");
    assert_eq!(body["roles"], serde_json::json!(["admin", "auditor"]));
    assert_eq!(body["tenant"], "acme");
}

#[tokio::test]
async fn claims_are_optional_when_the_field_is() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/bind/claim"))
        .bearer_auth(token(None))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["user"], "user-42");
    assert_eq!(body["tenant"], serde_json::Value::Null);
}

#[tokio::test]
async fn current_user_is_bound_from_claims() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/bind/user"))
        .bearer_auth(token(Some("acme")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["id"], "user-42");
    assert_eq!(body["name"], "Ada Lovelace");
    assert_eq!(body["email"], "ada@example.com");
    assert_eq!(body["tenant"], "acme");
    assert_eq!(body["roles"][0], "admin");
    assert_eq!(body["user_id"], "user-42");
    assert_eq!(body["user_name"], "Ada Lovelace");
    assert_eq!(body["is_admin"], true);
    assert_eq!(body["requires_ok"], true);
}

#[tokio::test]
async fn current_user_required_is_unauthorized_without_a_principal() {
    let base = spawn_host().await;
    let response = reqwest::get(format!("{base}/bind/user")).await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn optional_current_user_is_none_for_anonymous_requests() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::get(format!("{base}/bind/user-optional"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["user_id"], serde_json::Value::Null);
    assert_eq!(body["anonymous"], true);

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/bind/user-optional"))
        .bearer_auth(token(None))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["user_id"], "user-42");
    assert_eq!(body["anonymous"], false);
}

#[tokio::test]
async fn claim_binding_without_a_principal_is_unauthorized() {
    let base = spawn_host().await;
    let resp = reqwest::get(format!("{base}/bind/claim")).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn path_body_and_header_bind_together() {
    let base = spawn_host().await;
    let resp = reqwest::Client::new()
        .put(format!("{base}/bind/mixed/99"))
        .header("x-actor", "alice")
        .json(&serde_json::json!({ "name": "renamed" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "99");
    assert_eq!(body["name"], "renamed");
    assert_eq!(body["actor"], "alice");
}

#[tokio::test]
async fn missing_body_field_is_reported_not_defaulted() {
    let base = spawn_host().await;
    let resp = reqwest::Client::new()
        .put(format!("{base}/bind/mixed/99"))
        .header("x-actor", "alice")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 400);
    let problem: serde_json::Value = resp.json().await.unwrap();
    assert!(
        problem["errors"]["name"].is_array(),
        "expected a `name` error, got {problem}"
    );
}

#[tokio::test]
async fn query_wins_over_the_route_on_get() {
    let base = spawn_host().await;

    let from_path: serde_json::Value = reqwest::get(format!("{base}/bind/priority/from-path"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(from_path["key"], "from-path");

    let from_query: serde_json::Value =
        reqwest::get(format!("{base}/bind/priority/from-path?key=from-query"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(from_query["key"], "from-query");
}

#[tokio::test]
async fn body_is_authoritative_on_write_methods() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/bind/priority-body/7?key=from-query"))
        .json(&serde_json::json!({ "key": "from-body" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["id"], "7");
    assert_eq!(body["key"], "from-body");
}

#[tokio::test]
async fn composite_fields_bind_from_the_body() {
    let base = spawn_host().await;
    let resp = reqwest::Client::new()
        .put(format!("{base}/bind/complex/42"))
        .json(&serde_json::json!({
            "address": { "city": "Shanghai", "zip": "200000" },
            "items": [
                { "sku": "a-1", "qty": 1 },
                { "sku": "b-2", "qty": 2 }
            ],
            "meta": { "source": "web" }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "42");
    assert_eq!(body["city"], "Shanghai");
    assert_eq!(body["zip"], "200000");
    assert_eq!(body["items"], serde_json::json!(["a-1", "b-2"]));
    assert_eq!(body["qty_total"], 3);
    assert_eq!(body["meta"], "web");
}

#[tokio::test]
async fn explicit_custom_scalars_bind_from_path_and_query() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::get(format!("{base}/bind/custom/ABC?filter=XYZ"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["id"], "abc");
    assert_eq!(body["filter"], "xyz");
}
