//! WEBX-18: `#[authorize(dynamic)]` — macro metadata and end-to-end delegation.
//!
//! The `dynamic` flag keeps the endpoint's declared roles/permissions (they are
//! the dynamic authorizer's input) but skips the claim-snapshot gate: the static
//! check only requires an authenticated principal, and `IDynamicAuthorizer`
//! decides. These tests cover both halves of the chain:
//!
//! * macro → `RouteEntry` metadata (`authorization_dynamic`),
//! * `RouteEntry` → `Host` → `StubEndpoint` → registered authorizer (real HTTP).

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use webx::*;

// ── Endpoints whose metadata the macro must flag ─────────────────────────────

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct DynamicOnlyRequest;

#[post("/api/webx18/meta/dynamic-only")]
#[authorize(dynamic)]
impl IRequest<String> for DynamicOnlyRequest {}

#[derive(Default)]
struct DynamicOnlyHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DynamicOnlyRequest, String> for DynamicOnlyHandler {
    async fn handle(&mut self, _req: DynamicOnlyRequest) -> ActionResult<String> {
        Ok(String::new())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct DynamicCombinedRequest;

#[post("/api/webx18/meta/dynamic-combined")]
#[authorize(dynamic, permission = "orders:delete")]
impl IRequest<String> for DynamicCombinedRequest {}

#[derive(Default)]
struct DynamicCombinedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DynamicCombinedRequest, String> for DynamicCombinedHandler {
    async fn handle(&mut self, _req: DynamicCombinedRequest) -> ActionResult<String> {
        Ok(String::new())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct DynamicMixedRequest;

#[post("/api/webx18/meta/dynamic-mixed")]
#[authorize(role = "admin")]
#[authorize(dynamic, permission = "reports:export")]
impl IRequest<String> for DynamicMixedRequest {}

#[derive(Default)]
struct DynamicMixedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DynamicMixedRequest, String> for DynamicMixedHandler {
    async fn handle(&mut self, _req: DynamicMixedRequest) -> ActionResult<String> {
        Ok(String::new())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct StaticMetaRequest;

#[post("/api/webx18/meta/static")]
#[authorize(permission = "orders:delete")]
impl IRequest<String> for StaticMetaRequest {}

#[derive(Default)]
struct StaticMetaHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StaticMetaRequest, String> for StaticMetaHandler {
    async fn handle(&mut self, _req: StaticMetaRequest) -> ActionResult<String> {
        Ok(String::new())
    }
}

fn route_entry(path: &str) -> &'static RouteEntry {
    inventory::iter::<RouteEntry>()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("route {path} missing from inventory"))
}

#[test]
fn dynamic_marker_is_collected_from_the_attribute() {
    let entry = route_entry("/api/webx18/meta/dynamic-only");
    assert!(
        entry.authorization_dynamic,
        "`#[authorize(dynamic)]` must set the endpoint-level dynamic flag"
    );
    // The attribute still contributes its (empty = any authenticated) rule; the
    // dynamic authorizer receives it as the declaration.
    assert_eq!(entry.authorization.len(), 1);
    assert!(entry.authorization[0].roles.is_empty());
    assert!(entry.authorization[0].permissions.is_empty());
}

#[test]
fn dynamic_combines_with_role_and_permission_in_one_attribute() {
    let entry = route_entry("/api/webx18/meta/dynamic-combined");
    assert!(entry.authorization_dynamic);
    assert_eq!(entry.authorization.len(), 1);
    assert_eq!(entry.authorization[0].permissions, &["orders:delete"]);
}

#[test]
fn one_dynamic_attribute_makes_the_whole_endpoint_dynamic() {
    let entry = route_entry("/api/webx18/meta/dynamic-mixed");
    assert!(
        entry.authorization_dynamic,
        "mixing `#[authorize(dynamic, ..)]` with static attributes must make the whole endpoint dynamic"
    );
    let rules: Vec<(&[&str], &[&str])> = entry
        .authorization
        .iter()
        .map(|rule| (rule.roles, rule.permissions))
        .collect();
    assert_eq!(
        rules,
        vec![
            (&["admin"][..], &[][..]),
            (&[][..], &["reports:export"][..])
        ],
        "the declared rules stay intact (AND structure, declaration order)"
    );
}

#[test]
fn static_routes_keep_the_dynamic_flag_off() {
    let entry = route_entry("/api/webx18/meta/static");
    assert!(
        !entry.authorization_dynamic,
        "a plain `#[authorize(permission = ..)]` must stay on the static path"
    );
    assert_eq!(entry.authorization[0].permissions, &["orders:delete"]);
}

// ── End-to-end: the host hands a dynamic endpoint to the authorizer ─────────

const TEST_SECRET: &str = "webx18-dynamic-test-secret-key-32+";

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct AllowedRequest;

#[delete("/api/webx18/dynamic-allow")]
#[authorize(dynamic, permission = "orders:delete")]
impl IRequest<String> for AllowedRequest {}

#[derive(Default)]
struct AllowedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<AllowedRequest, String> for AllowedHandler {
    async fn handle(&mut self, _req: AllowedRequest) -> ActionResult<String> {
        Ok("dynamic-allowed".to_string())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct DeniedRequest;

#[delete("/api/webx18/dynamic-deny")]
#[authorize(dynamic, permission = "orders:delete")]
impl IRequest<String> for DeniedRequest {}

#[derive(Default)]
struct DeniedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<DeniedRequest, String> for DeniedHandler {
    async fn handle(&mut self, _req: DeniedRequest) -> ActionResult<String> {
        Ok("must-not-run".to_string())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct StaticRequest;

#[delete("/api/webx18/static")]
#[authorize(permission = "orders:delete")]
impl IRequest<String> for StaticRequest {}

#[derive(Default)]
struct StaticHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StaticRequest, String> for StaticHandler {
    async fn handle(&mut self, _req: StaticRequest) -> ActionResult<String> {
        Ok("static".to_string())
    }
}

/// Allows everything except the `/dynamic-deny` pattern, and records every
/// route it was asked about so the test can prove the static ones never reach it.
#[derive(Default)]
struct RecordingAuthorizer {
    calls: Mutex<Vec<String>>,
}

impl RecordingAuthorizer {
    fn routes(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl IDynamicAuthorizer for RecordingAuthorizer {
    async fn authorize(
        &self,
        _claims: &dyn IClaims,
        route_pattern: &str,
        _method: &str,
    ) -> Result<()> {
        self.calls.lock().unwrap().push(route_pattern.to_string());
        if route_pattern.ends_with("/dynamic-deny") {
            Err(Error::Forbidden(
                "dynamic authorizer denied the request".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn bearer_token(permissions: &[&str]) -> String {
    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        roles: Vec<String>,
        permissions: Vec<String>,
        exp: usize,
    }

    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize
        + 3600;
    let claims = TestClaims {
        sub: "user-1".to_string(),
        roles: vec!["user".to_string()],
        permissions: permissions.iter().map(|p| p.to_string()).collect(),
        exp,
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET.as_bytes()),
    )
    .unwrap()
}

async fn spawn_dynamic_host(port: u16, authorizer: Arc<RecordingAuthorizer>) {
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_authentication()
        .configure(|builder| {
            builder.useOptions(|options| {
                options.jwt.secret = TEST_SECRET.to_string();
            });
        })
        .register(move |svc| {
            let authorizer = Arc::clone(&authorizer);
            svc.singleton::<dyn IDynamicAuthorizer>(move |_| authorizer.clone())
        })
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

/// The token has **no** `permissions` claim value. Under the pre-1.3.0 static
/// gate that means 403 before any dynamic check; in dynamic mode the declaration
/// reaches the authorizer, which decides. The static route is the regression:
/// it still answers 403 from claims, without ever calling the authorizer.
#[tokio::test]
async fn dynamic_routes_delegate_to_the_authorizer_while_static_routes_do_not() {
    let port = free_port();
    let authorizer = Arc::new(RecordingAuthorizer::default());
    spawn_dynamic_host(port, Arc::clone(&authorizer)).await;

    let client = reqwest::Client::new();
    let token = bearer_token(&[]);
    let url = |path: &str| format!("http://127.0.0.1:{port}{path}");

    // ① dynamic + claims without the declared permission → authorizer allows.
    let allowed = client
        .delete(url("/api/webx18/dynamic-allow"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status().as_u16(), 200);
    let body: serde_json::Value = allowed.json().await.unwrap();
    assert_eq!(body, "dynamic-allowed");

    // ② dynamic + authorizer denial → 403 from the authorizer, not the gate.
    let denied = client
        .delete(url("/api/webx18/dynamic-deny"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status().as_u16(), 403);
    let body: serde_json::Value = denied.json().await.unwrap();
    assert_eq!(body["status"], 403);

    // ③ static route, same token: claims gate still rejects with the declaration.
    let static_resp = client
        .delete(url("/api/webx18/static"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(static_resp.status().as_u16(), 403);
    let body: serde_json::Value = static_resp.json().await.unwrap();
    assert_eq!(
        body["required_permissions"],
        serde_json::json!(["orders:delete"])
    );

    // ④ dynamic route without claims: still 401 (authentication is mandatory).
    let anonymous = client
        .delete(url("/api/webx18/dynamic-allow"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status().as_u16(), 401);

    let routes = authorizer.routes();
    assert!(
        routes
            .iter()
            .any(|route| route == "/api/webx18/dynamic-allow"),
        "dynamic allow route must reach the authorizer, got {routes:?}"
    );
    assert!(
        routes
            .iter()
            .any(|route| route == "/api/webx18/dynamic-deny"),
        "dynamic deny route must reach the authorizer, got {routes:?}"
    );
    assert!(
        !routes.iter().any(|route| route == "/api/webx18/static"),
        "the static gate must short-circuit before dynamic authorization, got {routes:?}"
    );
}
