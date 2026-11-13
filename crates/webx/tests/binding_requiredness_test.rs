//! Required/optional semantics for every binding source.
//!
//! Each cell of the matrix is checked end to end: a required field that is
//! absent returns 400 with the field name, an optional field is `None`, a list
//! is empty rather than missing, and `Option<Vec<T>>` distinguishes "absent"
//! from "present but empty".

use std::net::TcpListener;

use jsonwebtoken::{EncodingKey, Header};
use serde::{Deserialize, Serialize};
use webx::*;

const TEST_SECRET: &[u8] = b"requiredness-secret";

// ---------------------------------------------------------------------------
// Query: required, optional, list, optional list
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct QueryModel {
    a: String,
    b: Option<String>,
    c: Vec<u32>,
    d: Option<Vec<u32>>,
}

#[get("/req/query")]
impl IRequest<serde_json::Value> for QueryModel {}

#[derive(Default)]
struct QueryModelHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<QueryModel, serde_json::Value> for QueryModelHandler {
    async fn handle(&mut self, req: QueryModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "a": req.a,
            "b": req.b,
            "c": req.c,
            "d": req.d,
        }))
    }
}

// ---------------------------------------------------------------------------
// Header: required, optional, list
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct HeaderModel {
    #[frompath]
    id: String,
    #[fromheader("X-Required")]
    required: String,
    #[fromheader("X-Optional")]
    optional: Option<String>,
    #[fromheader("X-List")]
    list: Vec<u32>,
}

#[get("/req/header/{id}")]
impl IRequest<serde_json::Value> for HeaderModel {}

#[derive(Default)]
struct HeaderModelHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<HeaderModel, serde_json::Value> for HeaderModelHandler {
    async fn handle(&mut self, req: HeaderModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "required": req.required,
            "optional": req.optional,
            "list": req.list,
        }))
    }
}

// ---------------------------------------------------------------------------
// Claim: required, optional (plus unauthenticated)
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct ClaimModel {
    #[fromclaim("sub")]
    sub: String,
    #[fromclaim("roles")]
    roles: Vec<String>,
    #[fromclaim("tenant_id")]
    tenant: Option<String>,
}

#[get("/req/claim")]
impl IRequest<serde_json::Value> for ClaimModel {}

#[derive(Default)]
struct ClaimModelHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<ClaimModel, serde_json::Value> for ClaimModelHandler {
    async fn handle(&mut self, req: ClaimModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "sub": req.sub,
            "roles": req.roles,
            "tenant": req.tenant,
        }))
    }
}

// ---------------------------------------------------------------------------
// Form: required text, optional file, file list
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct FormModel {
    title: String,
    cover: Option<FormFile>,
    gallery: Vec<FormFile>,
    confirm: bool,
}

#[post("/req/form")]
impl IRequest<serde_json::Value> for FormModel {}

#[derive(Default)]
struct FormModelHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<FormModel, serde_json::Value> for FormModelHandler {
    async fn handle(&mut self, req: FormModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "title": req.title,
            "confirm": req.confirm,
            "cover": req.cover.map(|f| f.size()),
            "gallery": req.gallery.len(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Body: required, optional, serde default, list, optional list
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize, WebxRequestMeta)]
struct BodyModel {
    #[frompath]
    id: String,
    name: String,
    nickname: Option<String>,
    tags: Vec<String>,
    #[serde(default)]
    note: String,
    extra: Option<Vec<u32>>,
}

#[put("/req/body/{id}")]
impl IRequest<serde_json::Value> for BodyModel {}

#[derive(Default)]
struct BodyModelHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<BodyModel, serde_json::Value> for BodyModelHandler {
    async fn handle(&mut self, req: BodyModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "name": req.name,
            "nickname": req.nickname,
            "tags": req.tags,
            "note": req.note,
            "extra": req.extra,
        }))
    }
}

// ---------------------------------------------------------------------------
// Composite type on a method without a body
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize)]
#[allow(dead_code)]
struct Payload {
    value: String,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
#[allow(dead_code)]
struct CompositeGetModel {
    payload: Payload,
}

#[get("/req/composite")]
impl IRequest<serde_json::Value> for CompositeGetModel {}

#[derive(Default)]
struct CompositeGetHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CompositeGetModel, serde_json::Value> for CompositeGetHandler {
    async fn handle(&mut self, _req: CompositeGetModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({}))
    }
}

/// An *optional* composite field is just as unbindable on a bodyless method:
/// it must be a clear 400, never a silent `None`.
#[derive(Default, Deserialize, WebxRequestMeta)]
#[allow(dead_code)]
struct OptionalCompositeGetModel {
    payload: Option<Payload>,
}

#[get("/req/composite-optional")]
impl IRequest<serde_json::Value> for OptionalCompositeGetModel {}

#[derive(Default)]
struct OptionalCompositeGetHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<OptionalCompositeGetModel, serde_json::Value> for OptionalCompositeGetHandler {
    async fn handle(&mut self, _req: OptionalCompositeGetModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({}))
    }
}

// ---------------------------------------------------------------------------
// serde attributes on body fields
// ---------------------------------------------------------------------------

fn default_retries() -> u32 {
    3
}

#[derive(Default, Deserialize, WebxRequestMeta)]
#[serde(rename_all = "camelCase")]
struct RenamedModel {
    #[frompath]
    id: String,
    #[serde(alias = "oldName")]
    display_name: String,
    #[serde(default = "default_retries")]
    retries: u32,
}

#[put("/req/renamed/{id}")]
impl IRequest<serde_json::Value> for RenamedModel {}

#[derive(Default)]
struct RenamedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<RenamedModel, serde_json::Value> for RenamedHandler {
    async fn handle(&mut self, req: RenamedModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "display_name": req.display_name,
            "retries": req.retries,
        }))
    }
}

#[derive(Default, Deserialize)]
struct Paging {
    page: u32,
    size: u32,
}

#[derive(Default, Deserialize, WebxRequestMeta)]
struct FlattenModel {
    #[frompath]
    id: String,
    name: String,
    #[serde(flatten)]
    paging: Paging,
}

#[put("/req/flatten/{id}")]
impl IRequest<serde_json::Value> for FlattenModel {}

#[derive(Default)]
struct FlattenHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<FlattenModel, serde_json::Value> for FlattenHandler {
    async fn handle(&mut self, req: FlattenModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "id": req.id,
            "name": req.name,
            "page": req.paging.page,
            "size": req.paging.size,
        }))
    }
}

// ---------------------------------------------------------------------------
// Named sources, strict bodies and serde-aware body fields
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct NamedModel {
    #[frompath("id")]
    user_id: String,
    #[fromquery("q")]
    keyword: String,
}

#[get("/req/named/{id}")]
impl IRequest<serde_json::Value> for NamedModel {}

#[derive(Default)]
struct NamedModelHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<NamedModel, serde_json::Value> for NamedModelHandler {
    async fn handle(&mut self, req: NamedModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({ "user_id": req.user_id, "keyword": req.keyword }))
    }
}

#[derive(WebxRequestMeta)]
struct NamedUpload {
    #[fromfile("avatar")]
    picture: FormFile,
    #[fromform("title")]
    caption: String,
}

#[post("/req/named-upload")]
impl IRequest<serde_json::Value> for NamedUpload {}

#[derive(Default)]
struct NamedUploadHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<NamedUpload, serde_json::Value> for NamedUploadHandler {
    async fn handle(&mut self, req: NamedUpload) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({
            "size": req.picture.size(),
            "caption": req.caption,
        }))
    }
}

#[derive(Default, Deserialize, WebxRequestMeta)]
#[serde(deny_unknown_fields)]
struct StrictModel {
    #[frompath]
    id: String,
    name: String,
}

#[put("/req/strict/{id}")]
impl IRequest<serde_json::Value> for StrictModel {}

#[derive(Default)]
struct StrictHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StrictModel, serde_json::Value> for StrictHandler {
    async fn handle(&mut self, req: StrictModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({ "id": req.id, "name": req.name }))
    }
}

#[derive(Default, Deserialize, WebxRequestMeta)]
struct NullModel {
    #[frompath]
    id: String,
    name: String,
    nickname: Option<String>,
}

#[put("/req/null/{id}")]
impl IRequest<serde_json::Value> for NullModel {}

#[derive(Default)]
struct NullHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<NullModel, serde_json::Value> for NullHandler {
    async fn handle(&mut self, req: NullModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({ "id": req.id, "name": req.name, "nickname": req.nickname }))
    }
}

fn deserialize_upper<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(value.to_ascii_uppercase())
}

#[derive(Default, Deserialize, WebxRequestMeta)]
struct CustomSerdeModel {
    #[frompath]
    id: String,
    #[serde(deserialize_with = "deserialize_upper")]
    name: String,
}

#[put("/req/custom-serde/{id}")]
impl IRequest<serde_json::Value> for CustomSerdeModel {}

#[derive(Default)]
struct CustomSerdeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CustomSerdeModel, serde_json::Value> for CustomSerdeHandler {
    async fn handle(&mut self, req: CustomSerdeModel) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({ "id": req.id, "name": req.name }))
    }
}

// ---------------------------------------------------------------------------
// OpenAPI metadata for a qualified `self_ty` and same-named request types
// ---------------------------------------------------------------------------

/// Two request types with the *same bare name* in different modules. The OpenAPI
/// metadata must be keyed by `TypeId`, not by the type name string.
mod alpha {
    use super::*;

    #[derive(Default, Deserialize, WebxRequestMeta)]
    pub struct DetailRequest {
        #[fromquery]
        pub alpha_marker: String,
    }
}

mod beta {
    use super::*;

    #[derive(Default, Deserialize, WebxRequestMeta)]
    pub struct DetailRequest {
        #[fromquery]
        pub beta_marker: String,
    }
}

// Both endpoints write a *qualified* `self_ty`, so the route records
// `alpha::DetailRequest` / `beta::DetailRequest` while the derive records the
// bare `DetailRequest` — a name lookup would miss both.
#[get("/req/alpha-detail")]
impl IRequest<serde_json::Value> for alpha::DetailRequest {}

#[derive(Default)]
struct AlphaDetailHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<alpha::DetailRequest, serde_json::Value> for AlphaDetailHandler {
    async fn handle(&mut self, req: alpha::DetailRequest) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({ "alpha_marker": req.alpha_marker }))
    }
}

#[get("/req/beta-detail")]
impl IRequest<serde_json::Value> for beta::DetailRequest {}

#[derive(Default)]
struct BetaDetailHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<beta::DetailRequest, serde_json::Value> for BetaDetailHandler {
    async fn handle(&mut self, req: beta::DetailRequest) -> ActionResult<serde_json::Value> {
        Ok(serde_json::json!({ "beta_marker": req.beta_marker }))
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct TokenClaims {
    sub: String,
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
    jsonwebtoken::encode(
        &Header::default(),
        &TokenClaims {
            sub: "user-7".to_string(),
            roles: vec!["admin".to_string()],
            permissions: Vec::new(),
            exp,
            tenant_id: tenant.map(str::to_string),
        },
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
    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

async fn problem_errors(response: reqwest::Response) -> serde_json::Value {
    let problem: serde_json::Value = response.json().await.unwrap();
    problem["errors"].clone()
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_required_optional_and_lists() {
    let base = spawn_host().await;

    let minimal: serde_json::Value = reqwest::get(format!("{base}/req/query?a=x"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(minimal["b"], serde_json::Value::Null);
    assert_eq!(minimal["c"], serde_json::json!([]));
    assert_eq!(minimal["d"], serde_json::Value::Null);

    let full: serde_json::Value = reqwest::get(format!("{base}/req/query?a=x&b=y&c=1&c=2&d=3,4"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(full["b"], "y");
    assert_eq!(full["c"], serde_json::json!([1, 2]));
    assert_eq!(full["d"], serde_json::json!([3, 4]));
}

#[tokio::test]
async fn query_missing_required_field_is_reported() {
    let base = spawn_host().await;
    let response = reqwest::get(format!("{base}/req/query?b=y")).await.unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(errors["a"].is_array(), "expected `a`, got {errors}");
}

#[tokio::test]
async fn query_invalid_list_element_is_reported() {
    let base = spawn_host().await;
    let response = reqwest::get(format!("{base}/req/query?a=x&c=1&c=oops"))
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(errors["c"].is_array(), "expected `c`, got {errors}");
}

#[tokio::test]
async fn query_present_but_empty_list_is_not_a_parse_error() {
    let base = spawn_host().await;

    // `?c=` / `?d=` mean "present, no values": an empty list, and
    // `Option<Vec<T>>` distinguishes that (`Some([])`) from absent (`None`).
    let empty: serde_json::Value = reqwest::get(format!("{base}/req/query?a=x&c=&d="))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(empty["c"], serde_json::json!([]));
    assert_eq!(empty["d"], serde_json::json!([]));

    // Repeated keys still bind every occurrence.
    let repeated: serde_json::Value = reqwest::get(format!("{base}/req/query?a=x&d=1&d=2"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repeated["d"], serde_json::json!([1, 2]));
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn header_required_optional_and_list() {
    let base = spawn_host().await;

    let minimal: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/req/header/1"))
        .header("X-Required", "yes")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(minimal["optional"], serde_json::Value::Null);
    assert_eq!(minimal["list"], serde_json::json!([]));

    let full: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/req/header/1"))
        .header("X-Required", "yes")
        .header("X-Optional", "maybe")
        .header("X-List", "7, 8")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(full["optional"], "maybe");
    assert_eq!(full["list"], serde_json::json!([7, 8]));
}

#[tokio::test]
async fn header_missing_required_field_is_reported() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .get(format!("{base}/req/header/1"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(
        errors["required"].is_array(),
        "expected `required`, got {errors}"
    );
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

#[tokio::test]
async fn claim_required_optional_and_roles() {
    let base = spawn_host().await;

    let without_tenant: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/req/claim"))
        .bearer_auth(token(None))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(without_tenant["sub"], "user-7");
    assert_eq!(without_tenant["roles"], serde_json::json!(["admin"]));
    assert_eq!(without_tenant["tenant"], serde_json::Value::Null);

    let with_tenant: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/req/claim"))
        .bearer_auth(token(Some("acme")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(with_tenant["tenant"], "acme");
}

#[tokio::test]
async fn claim_without_a_principal_is_unauthorized() {
    let base = spawn_host().await;
    let response = reqwest::get(format!("{base}/req/claim")).await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

// ---------------------------------------------------------------------------
// Form
// ---------------------------------------------------------------------------

#[tokio::test]
async fn form_required_text_and_optional_files() {
    let base = spawn_host().await;

    let form = reqwest::multipart::Form::new()
        .text("title", "hello")
        .text("confirm", "true");
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/req/form"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["title"], "hello");
    assert_eq!(body["confirm"], true);
    assert_eq!(body["cover"], serde_json::Value::Null);
    assert_eq!(body["gallery"], 0);
}

#[tokio::test]
async fn form_missing_required_text_is_reported() {
    let base = spawn_host().await;
    let form = reqwest::multipart::Form::new().text("confirm", "true");
    let response = reqwest::Client::new()
        .post(format!("{base}/req/form"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(errors["title"].is_array(), "expected `title`, got {errors}");
}

// ---------------------------------------------------------------------------
// Body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn body_required_optional_default_and_lists() {
    let base = spawn_host().await;

    let minimal: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/body/5"))
        .json(&serde_json::json!({ "name": "n", "tags": [] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(minimal["id"], "5");
    assert_eq!(minimal["nickname"], serde_json::Value::Null);
    assert_eq!(minimal["tags"], serde_json::json!([]));
    assert_eq!(minimal["note"], "");
    assert_eq!(minimal["extra"], serde_json::Value::Null);

    // `Option<Vec<T>>` distinguishes "absent" (null) from "present, empty" ([]).
    let present_empty: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/body/5"))
        .json(&serde_json::json!({ "name": "n", "tags": [], "extra": [] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(present_empty["extra"], serde_json::json!([]));
}

#[tokio::test]
async fn body_missing_required_field_is_reported() {
    let base = spawn_host().await;

    let response = reqwest::Client::new()
        .put(format!("{base}/req/body/5"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(errors["name"].is_array(), "expected `name`, got {errors}");

    // Collections are optional on every source: an absent list is empty.
    let response = reqwest::Client::new()
        .put(format!("{base}/req/body/5"))
        .json(&serde_json::json!({ "name": "n" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["tags"], serde_json::json!([]));
}

#[tokio::test]
async fn body_falls_back_to_the_query_string() {
    let base = spawn_host().await;

    // Neither `name` (required) nor `nickname` (optional) is in the body.
    let body: serde_json::Value = reqwest::Client::new()
        .put(format!(
            "{base}/req/body/5?name=from-query&nickname=from-query"
        ))
        .json(&serde_json::json!({ "tags": [] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["name"], "from-query");
    assert_eq!(body["nickname"], "from-query");
}

#[tokio::test]
async fn body_wins_over_the_query_string() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/body/5?name=from-query"))
        .json(&serde_json::json!({ "name": "from-body", "tags": [] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["name"], "from-body");
}

#[tokio::test]
async fn form_falls_back_to_the_query_string() {
    let base = spawn_host().await;

    // `title` is absent from the form; `confirm` is present.
    let form = reqwest::multipart::Form::new().text("confirm", "true");
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/req/form?title=from-query"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["title"], "from-query");
    assert_eq!(body["confirm"], true);
}

// ---------------------------------------------------------------------------
// Composite types without a body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn composite_field_without_a_body_is_a_clear_400() {
    let base = spawn_host().await;
    let response = reqwest::get(format!("{base}/req/composite")).await.unwrap();

    assert_eq!(response.status().as_u16(), 400);
    let problem: serde_json::Value = response.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("payload") && detail.contains("composite"),
        "expected a composite-field explanation, got {problem}"
    );
}

#[tokio::test]
async fn optional_composite_field_without_a_body_is_a_clear_400() {
    let base = spawn_host().await;
    let response = reqwest::get(format!("{base}/req/composite-optional"))
        .await
        .unwrap();

    assert_eq!(response.status().as_u16(), 400);
    let problem: serde_json::Value = response.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("payload") && detail.contains("composite"),
        "expected a composite-field explanation, got {problem}"
    );
}

#[tokio::test]
async fn body_honours_rename_all_and_serde_defaults() {
    let base = spawn_host().await;

    let body: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/renamed/1"))
        .json(&serde_json::json!({ "displayName": "Ada" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["display_name"], "Ada");
    assert_eq!(body["retries"], 3);

    // `#[serde(alias)]` keys are accepted too.
    let body: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/renamed/1"))
        .json(&serde_json::json!({ "oldName": "Grace" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["display_name"], "Grace");

    let response = reqwest::Client::new()
        .put(format!("{base}/req/renamed/1"))
        .json(&serde_json::json!({ "retries": 5 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(
        errors["display_name"].is_array(),
        "expected `display_name`, got {errors}"
    );
}

#[tokio::test]
async fn flattened_body_fields_bind() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/flatten/1"))
        .json(&serde_json::json!({ "name": "n", "page": 2, "size": 20 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["name"], "n");
    assert_eq!(body["page"], 2);
    assert_eq!(body["size"], 20);
}

#[tokio::test]
async fn sources_accept_explicit_keys() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::get(format!("{base}/req/named/7?q=hello"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["user_id"], "7");
    assert_eq!(body["keyword"], "hello");
}

#[tokio::test]
async fn files_and_form_parts_accept_explicit_keys() {
    let base = spawn_host().await;
    let form = reqwest::multipart::Form::new()
        .part(
            "avatar",
            reqwest::multipart::Part::bytes(b"payload".to_vec()).file_name("a.bin"),
        )
        .text("title", "caption");
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/req/named-upload"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["size"], 7);
    assert_eq!(body["caption"], "caption");
}

#[tokio::test]
async fn strict_bodies_reject_unknown_keys() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .put(format!("{base}/req/strict/1"))
        .json(&serde_json::json!({ "name": "n", "extra": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let problem: serde_json::Value = response.json().await.unwrap();
    let detail = problem["errors"][""].as_array().expect("body error");
    assert!(
        detail[0].as_str().unwrap_or_default().contains("extra"),
        "expected the unknown key, got {problem}"
    );
}

#[tokio::test]
async fn null_is_not_absence_for_a_required_field() {
    let base = spawn_host().await;
    let response = reqwest::Client::new()
        .put(format!("{base}/req/null/1?name=from-query"))
        .json(&serde_json::json!({ "name": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let errors = problem_errors(response).await;
    assert!(errors["name"].is_array(), "expected `name`, got {errors}");
}

#[tokio::test]
async fn custom_deserializers_apply_to_body_fields() {
    let base = spawn_host().await;
    let body: serde_json::Value = reqwest::Client::new()
        .put(format!("{base}/req/custom-serde/1"))
        .json(&serde_json::json!({ "name": "abc" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["name"], "ABC");
}

// ---------------------------------------------------------------------------
// OpenAPI mirrors the runtime requiredness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openapi_required_flags_match_runtime() {
    let base = spawn_host().await;
    let spec: serde_json::Value = reqwest::get(format!("{base}/api/openapi.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let parameters = spec["paths"]["/req/query"]["get"]["parameters"]
        .as_array()
        .expect("query parameters");
    let flag = |name: &str| {
        parameters
            .iter()
            .find(|p| p["name"] == name)
            .map(|p| p["required"].as_bool().unwrap_or(false))
            .unwrap_or_else(|| panic!("missing parameter {name}"))
    };
    assert!(flag("a"));
    assert!(!flag("b"));
    assert!(!flag("c"));
    assert!(!flag("d"));

    let header_parameters = spec["paths"]["/req/header/{id}"]["get"]["parameters"]
        .as_array()
        .expect("header parameters");
    let header_flag = |name: &str| {
        header_parameters
            .iter()
            .find(|p| p["name"] == name)
            .map(|p| p["required"].as_bool().unwrap_or(false))
            .unwrap_or_else(|| panic!("missing header {name}"))
    };
    assert!(header_flag("X-Required"));
    assert!(!header_flag("X-Optional"));
    assert!(!header_flag("X-List"));
}

#[tokio::test]
async fn openapi_documents_json_body_fields() {
    let base = spawn_host().await;
    let spec: serde_json::Value = reqwest::get(format!("{base}/api/openapi.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // A write endpoint documents each body field as a property of the body.
    let request_body = &spec["paths"]["/req/body/{id}"]["put"]["requestBody"];
    assert_eq!(request_body["required"], true, "got {request_body}");
    let schema = &request_body["content"]["application/json"]["schema"];
    assert_eq!(schema["type"], "object", "got {request_body}");
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["nickname"]["type"], "string");
    assert_eq!(schema["properties"]["tags"]["type"], "array");
    assert_eq!(schema["properties"]["note"]["type"], "string");
    assert_eq!(schema["properties"]["extra"]["type"], "array");
    // Only the non-`Option`, no-`#[serde(default)]` field is required.
    assert_eq!(schema["required"], serde_json::json!(["name"]));
    assert!(request_body["content"]["multipart/form-data"].is_null());

    // A read method has no body: body-only fields must not fake one.
    assert!(spec["paths"]["/req/composite"]["get"]["requestBody"].is_null());
    assert!(spec["paths"]["/req/composite-optional"]["get"]["requestBody"].is_null());

    // Multipart endpoints keep their form schema and gain no JSON body.
    let form_body = &spec["paths"]["/req/form"]["post"]["requestBody"];
    assert_eq!(
        form_body["content"]["multipart/form-data"]["schema"]["type"],
        "object"
    );
    assert!(form_body["content"]["application/json"].is_null());
}

#[tokio::test]
async fn openapi_keeps_parameters_for_qualified_and_same_named_requests() {
    let base = spawn_host().await;
    let spec: serde_json::Value = reqwest::get(format!("{base}/api/openapi.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let parameter_names = |path: &str| -> Vec<String> {
        spec["paths"][path]["get"]["parameters"]
            .as_array()
            .map(|parameters| {
                parameters
                    .iter()
                    .map(|param| param["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Qualified `self_ty`: the route name is `alpha::DetailRequest` while the
    // derive registered the bare `DetailRequest`; TypeId linking keeps it.
    assert_eq!(parameter_names("/req/alpha-detail"), vec!["alpha_marker"]);

    // Same-named request types in two modules never share their parameters.
    assert_eq!(parameter_names("/req/beta-detail"), vec!["beta_marker"]);
}
