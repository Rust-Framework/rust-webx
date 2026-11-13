//! Tests for StubEndpoint authorization checks and dispatch lifecycle.
//!
//! Uses the mock `TestHttpContext` from `test_utils.rs`.

mod test_utils;

use std::collections::HashMap;
use std::sync::Arc;
use webx_core::action::{JsonResult, Response};
use webx_core::auth::{DeclaredRules, IClaims, IDynamicAuthorizer};
use webx_core::error::{Error, Result as LrwfResult};
use webx_core::http::{IClaimsExt, IHttpContext};
use webx_core::route::scan::AuthorizeRule;
use webx_core::routing::IEndpoint;
use webx_host::authz::AuthorizerSet;
use webx_host::endpoint::StubEndpoint;

const ADMIN_RULE: &[AuthorizeRule] = &[AuthorizeRule {
    roles: &["admin"],
    permissions: &[],
}];
const AUTHENTICATED_RULE: &[AuthorizeRule] = &[AuthorizeRule::authenticated()];
const SETTINGS_WRITE_RULE: &[AuthorizeRule] = &[AuthorizeRule {
    roles: &[],
    permissions: &["settings:write"],
}];

// â”€â”€ Mock IClaims â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

struct MockClaims {
    sub: String,
    roles: Vec<String>,
    permissions: Vec<String>,
}

impl IClaims for MockClaims {
    fn subject(&self) -> &str {
        &self.sub
    }
    fn roles(&self) -> &[String] {
        &self.roles
    }
    fn permissions(&self) -> &[String] {
        &self.permissions
    }
    fn claims(&self) -> &HashMap<String, String> {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }
    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(MockClaims {
            sub: self.sub.clone(),
            roles: self.roles.clone(),
            permissions: self.permissions.clone(),
        })
    }
}

// â”€â”€ Mock dispatch function â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn mock_dispatch_ok(
    _ctx: &mut dyn IHttpContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = LrwfResult<Response>> + Send + '_>> {
    Box::pin(
        async move { Ok(Box::new(JsonResult::new(serde_json::json!({"ok": true}))) as Response) },
    )
}

// â”€â”€ Mock dynamic authorizers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

struct DenyAllAuthorizer;

#[async_trait::async_trait]
impl IDynamicAuthorizer for DenyAllAuthorizer {
    async fn authorize(
        &self,
        _claims: &dyn IClaims,
        resource_key: &str,
        _method: &str,
    ) -> LrwfResult<()> {
        Err(Error::Http(format!(
            "Forbidden: access denied to {}",
            resource_key
        )))
    }
}

struct AllowAllAuthorizer;

#[async_trait::async_trait]
impl IDynamicAuthorizer for AllowAllAuthorizer {
    async fn authorize(
        &self,
        _claims: &dyn IClaims,
        _resource_key: &str,
        _method: &str,
    ) -> LrwfResult<()> {
        Ok(())
    }
}

// â”€â”€ Tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
async fn endpoint_normal_dispatch_returns_200() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/test",
        handler_type: "TestRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: &[],
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");
    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 200);
    assert!(body.is_some());
    let body_json: serde_json::Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert_eq!(body_json["ok"], true);
}

#[tokio::test]
async fn endpoint_no_dispatch_fn_returns_not_implemented() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/stub",
        handler_type: "MissingHandler",
        dispatch_fn: None,
        authorization: &[],
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/stub");
    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 501);
    let body_text = String::from_utf8(body.unwrap()).unwrap();
    assert!(body_text.contains("MissingHandler"));
    assert!(body_text.contains("/stub"));
}

#[tokio::test]
async fn endpoint_auth_required_with_valid_claims_succeeds() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/admin");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn endpoint_auth_required_without_claims_bubbles_unauthorized_error() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/admin");
    let result = endpoint.handle(&mut ctx).await;

    // Single 401 write path: the endpoint does not write the 401 itself, it
    // returns `Error::Unauthorized` so the host writes the status, the
    // problem+json body and the `WWW-Authenticate` challenge in one place.
    let err = result.expect_err("missing claims must bubble as Error::Unauthorized");
    assert!(
        matches!(&err, Error::Unauthorized(detail) if detail == "authentication required"),
        "expected Error::Unauthorized(\"authentication required\"), got {err:?}"
    );

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(
        status, 200,
        "endpoint must leave the response untouched for the host to write"
    );
    assert!(
        body.is_none(),
        "endpoint must not write its own 401 body (single write path)"
    );
}

#[tokio::test]
async fn endpoint_auth_required_wrong_role_returns_403() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/admin");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["user".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 403);
    let body_json: serde_json::Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert_eq!(body_json["required_roles"], serde_json::json!(["admin"]));
}

#[tokio::test]
async fn endpoint_auth_with_authenticated_role_allows_any_valid_jwt() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/profile",
        handler_type: "ProfileRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: AUTHENTICATED_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/profile");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec![],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn endpoint_dynamic_authorizer_denies_access() {
    let authorizers = Arc::new(AuthorizerSet::new(vec![Arc::new(DenyAllAuthorizer)]));
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/secret",
        handler_type: "SecretRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_RULE,
        authorization_dynamic: false,
        authorizers: Some(authorizers),
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/secret");
    *ctx.request_mut().route_pattern_mut() = Some("/secret".into());
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Forbidden"));
}

#[tokio::test]
async fn endpoint_dynamic_authorizer_allows_access() {
    let authorizers = Arc::new(AuthorizerSet::new(vec![Arc::new(AllowAllAuthorizer)]));
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/allowed",
        handler_type: "AllowedRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_RULE,
        authorization_dynamic: false,
        authorizers: Some(authorizers),
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/allowed");
    *ctx.request_mut().route_pattern_mut() = Some("/allowed".into());
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn endpoint_no_authorizer_skips_dynamic_check() {
    // When authorizers is None, the dynamic check is skipped entirely
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/public-admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/public-admin");
    *ctx.request_mut().route_pattern_mut() = Some("/public-admin".into());
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn endpoint_permission_required_with_valid_permission_succeeds() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/settings",
        handler_type: "SettingsRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: SETTINGS_WRITE_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/settings");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec![],
        permissions: vec!["settings:write".into()],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn endpoint_permission_required_missing_permission_returns_403() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/settings",
        handler_type: "SettingsRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: SETTINGS_WRITE_RULE,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/settings");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 403);
    let body_json: serde_json::Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert_eq!(
        body_json["required_permissions"],
        serde_json::json!(["settings:write"])
    );
}

const ADMIN_AND_EXPORT: &[AuthorizeRule] = &[
    AuthorizeRule {
        roles: &["admin"],
        permissions: &[],
    },
    AuthorizeRule {
        roles: &[],
        permissions: &["reports:export"],
    },
];

const ADMIN_OR_AUDITOR: &[AuthorizeRule] = &[AuthorizeRule {
    roles: &["admin", "auditor"],
    permissions: &[],
}];

#[tokio::test]
async fn multiple_authorize_rules_must_all_pass() {
    let endpoint = StubEndpoint {
        method: "POST",
        path: "/reports/export",
        handler_type: "ExportRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_AND_EXPORT,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("POST", "/reports/export");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec![],
    }));

    let (status, _, body) = {
        let result = endpoint.handle(&mut ctx).await;
        assert!(result.is_ok());
        ctx.into_response_parts()
    };
    assert_eq!(status, 403);
    let body_json: serde_json::Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert_eq!(
        body_json["required_permissions"],
        serde_json::json!(["reports:export"])
    );
}

#[tokio::test]
async fn roles_list_is_any_of() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/reports",
        handler_type: "ReportsRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: ADMIN_OR_AUDITOR,
        authorization_dynamic: false,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/reports");
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["auditor".into()],
        permissions: vec![],
    }));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

// ── WEBX-1: dynamic authorizers receive the endpoint's declared rules ───────

/// One `#[authorize]` rule as seen by the authorizer: (any-of roles, any-of
/// permissions). The list keeps the AND structure between rules.
type DeclaredRuleSummary = (Vec<String>, Vec<String>);

/// Records the `DeclaredRules` it receives, so a test can assert the endpoint's
/// `#[authorize]` declaration (permission/role literals) reached the backend.
#[derive(Default)]
struct RecordingAuthorizer {
    seen: std::sync::Mutex<Vec<DeclaredRuleSummary>>,
}

impl RecordingAuthorizer {
    fn declarations(&self) -> Vec<DeclaredRuleSummary> {
        self.seen.lock().unwrap().clone()
    }
}

/// Flatten `DeclaredRules` into one `(roles, permissions)` summary per rule.
fn summarize(declared: &DeclaredRules<'_>) -> Vec<DeclaredRuleSummary> {
    let roles: Vec<Vec<String>> = declared
        .roles()
        .map(|codes| codes.iter().map(|code| code.to_string()).collect())
        .collect();
    let permissions: Vec<Vec<String>> = declared
        .permissions()
        .map(|codes| codes.iter().map(|code| code.to_string()).collect())
        .collect();

    roles.into_iter().zip(permissions).collect()
}

#[async_trait::async_trait]
impl IDynamicAuthorizer for RecordingAuthorizer {
    async fn authorize(
        &self,
        _claims: &dyn IClaims,
        _route_pattern: &str,
        _method: &str,
    ) -> LrwfResult<()> {
        // Must not run: this authorizer overrides the rules-aware method.
        panic!("authorize() must not be used when authorize_with_rules is overridden");
    }

    async fn authorize_with_rules(
        &self,
        _claims: &dyn IClaims,
        _route_pattern: &str,
        _method: &str,
        declared: &DeclaredRules<'_>,
    ) -> LrwfResult<()> {
        self.seen.lock().unwrap().extend(summarize(declared));
        Ok(())
    }
}

fn recording_endpoint(
    authorizer: Arc<RecordingAuthorizer>,
    authorization: &'static [AuthorizeRule],
) -> StubEndpoint {
    StubEndpoint {
        method: "POST",
        path: "/orders",
        handler_type: "CreateOrderRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization,
        authorization_dynamic: false,
        authorizers: Some(Arc::new(AuthorizerSet::new(vec![authorizer]))),
    }
}

#[tokio::test]
async fn endpoint_dynamic_authorizer_receives_declared_permission_literal() {
    let authorizer = Arc::new(RecordingAuthorizer::default());
    let endpoint = recording_endpoint(Arc::clone(&authorizer), SETTINGS_WRITE_RULE);

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    *ctx.request_mut().route_pattern_mut() = Some("/orders".into());
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec![],
        permissions: vec!["settings:write".into()],
    }));

    endpoint.handle(&mut ctx).await.expect("must be allowed");

    assert_eq!(
        authorizer.declarations(),
        vec![(vec![], vec!["settings:write".to_string()])],
        "the `permission = \"settings:write\"` literal must reach the dynamic authorizer"
    );
}

#[tokio::test]
async fn endpoint_dynamic_authorizer_receives_empty_slices_when_nothing_declared() {
    let authorizer = Arc::new(RecordingAuthorizer::default());
    let endpoint = recording_endpoint(Arc::clone(&authorizer), AUTHENTICATED_RULE);

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    *ctx.request_mut().route_pattern_mut() = Some("/orders".into());
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec![],
        permissions: vec![],
    }));

    endpoint.handle(&mut ctx).await.expect("must be allowed");

    assert_eq!(
        authorizer.declarations(),
        vec![(Vec::<String>::new(), Vec::<String>::new())],
        "an endpoint without role/permission declarations must yield empty slices"
    );
}

#[tokio::test]
async fn endpoint_dynamic_authorizer_receives_and_structure_of_multiple_rules() {
    let authorizer = Arc::new(RecordingAuthorizer::default());
    let endpoint = recording_endpoint(Arc::clone(&authorizer), ADMIN_AND_EXPORT);

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    *ctx.request_mut().route_pattern_mut() = Some("/orders".into());
    ctx.set_claims(Box::new(MockClaims {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        permissions: vec!["reports:export".into()],
    }));

    endpoint.handle(&mut ctx).await.expect("must be allowed");

    assert_eq!(
        authorizer.declarations(),
        vec![
            (vec!["admin".to_string()], vec![]),
            (vec![], vec!["reports:export".to_string()]),
        ],
        "every `#[authorize]` attribute must stay a separate (ANDed) rule, not be flattened"
    );
}

// ── WEBX-18: `#[authorize(dynamic)]` delegates the whole declaration ────────

/// Records the exact `&'static [AuthorizeRule]` slice the primary path receives
/// (WEBX-19) and fails if the legacy methods run.
#[derive(Default)]
struct SliceRecordingAuthorizer {
    seen: std::sync::Mutex<Vec<&'static [AuthorizeRule]>>,
}

impl SliceRecordingAuthorizer {
    fn seen(&self) -> Vec<&'static [AuthorizeRule]> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl IDynamicAuthorizer for SliceRecordingAuthorizer {
    async fn authorize(
        &self,
        _claims: &dyn IClaims,
        _route_pattern: &str,
        _method: &str,
    ) -> LrwfResult<()> {
        panic!("authorize() must not run when authorize_with_declared_rules is overridden");
    }

    async fn authorize_with_declared_rules(
        &self,
        _claims: &dyn IClaims,
        _route_pattern: &str,
        _method: &str,
        declared: &'static [AuthorizeRule],
    ) -> LrwfResult<()> {
        self.seen.lock().unwrap().push(declared);
        Ok(())
    }
}

struct ForbiddenAuthorizer;

#[async_trait::async_trait]
impl IDynamicAuthorizer for ForbiddenAuthorizer {
    async fn authorize(
        &self,
        _claims: &dyn IClaims,
        _route_pattern: &str,
        _method: &str,
    ) -> LrwfResult<()> {
        Err(Error::Forbidden("dynamic denial".to_string()))
    }
}

/// Dynamic endpoint with claims that hold none of the declared permissions: the
/// static gate must be skipped and the declaration handed to the authorizer.
fn dynamic_endpoint(
    authorizers: Arc<AuthorizerSet>,
    authorization: &'static [AuthorizeRule],
) -> StubEndpoint {
    StubEndpoint {
        method: "POST",
        path: "/orders",
        handler_type: "CreateOrderRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization,
        authorization_dynamic: true,
        authorizers: Some(authorizers),
    }
}

fn claims_without_permissions() -> MockClaims {
    MockClaims {
        sub: "user-1".into(),
        roles: vec!["user".into()],
        permissions: vec![],
    }
}

#[tokio::test]
async fn dynamic_endpoint_skips_the_static_gate_and_passes_the_raw_declaration() {
    let authorizer = Arc::new(SliceRecordingAuthorizer::default());
    let endpoint = dynamic_endpoint(
        Arc::new(AuthorizerSet::new(vec![
            authorizer.clone() as Arc<dyn IDynamicAuthorizer>
        ])),
        ADMIN_AND_EXPORT,
    );

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    *ctx.request_mut().route_pattern_mut() = Some("/orders".into());
    // No `reports:export` permission: the static gate would have answered 403.
    ctx.set_claims(Box::new(claims_without_permissions()));

    endpoint
        .handle(&mut ctx)
        .await
        .expect("the dynamic authorizer decides, and it allows");
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200, "dynamic delegation must let the request through");

    let seen = authorizer.seen();
    assert_eq!(
        seen.len(),
        1,
        "exactly one authorization call for one request"
    );
    assert_eq!(
        seen[0], ADMIN_AND_EXPORT,
        "the whole ANDed declaration reaches the authorizer, literals included"
    );
    assert!(
        std::ptr::eq(seen[0].as_ptr(), ADMIN_AND_EXPORT.as_ptr()),
        "the route metadata slice is passed through as-is (zero allocation)"
    );
}

#[tokio::test]
async fn dynamic_endpoint_denied_by_the_authorizer_returns_403() {
    let endpoint = dynamic_endpoint(
        Arc::new(AuthorizerSet::new(vec![Arc::new(ForbiddenAuthorizer)])),
        SETTINGS_WRITE_RULE,
    );

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    *ctx.request_mut().route_pattern_mut() = Some("/orders".into());
    ctx.set_claims(Box::new(claims_without_permissions()));

    let err = endpoint
        .handle(&mut ctx)
        .await
        .expect_err("the authorizer denies, so the endpoint must bubble the error");
    assert_eq!(err.status_code(), 403);
    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(
        status, 200,
        "the endpoint must leave error writing to the host (single write path)"
    );
    assert!(body.is_none(), "the denied handler must not have run");
}

#[tokio::test]
async fn dynamic_endpoint_without_claims_returns_401() {
    let endpoint = dynamic_endpoint(
        Arc::new(AuthorizerSet::new(vec![Arc::new(AllowAllAuthorizer)])),
        SETTINGS_WRITE_RULE,
    );

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    let err = endpoint
        .handle(&mut ctx)
        .await
        .expect_err("dynamic mode does not make a route anonymous");
    assert!(
        matches!(&err, Error::Unauthorized(detail) if detail == "authentication required"),
        "expected Error::Unauthorized, got {err:?}"
    );
    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 200);
    assert!(body.is_none());
}

#[tokio::test]
async fn dynamic_endpoint_without_registered_authorizer_is_pass_through() {
    // Documented 1.3.0 behaviour: no `IDynamicAuthorizer` registered means no
    // dynamic checks; the endpoint only requires authentication. Applications
    // must not rely on the static gate while using `dynamic`.
    let endpoint = StubEndpoint {
        method: "POST",
        path: "/orders",
        handler_type: "CreateOrderRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: SETTINGS_WRITE_RULE,
        authorization_dynamic: true,
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    ctx.set_claims(Box::new(claims_without_permissions()));

    endpoint
        .handle(&mut ctx)
        .await
        .expect("no authorizer registered means pass-through");
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn static_endpoint_never_reaches_the_dynamic_authorizer_on_403() {
    let authorizer = Arc::new(SliceRecordingAuthorizer::default());
    let endpoint = StubEndpoint {
        method: "POST",
        path: "/orders",
        handler_type: "CreateOrderRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: SETTINGS_WRITE_RULE,
        authorization_dynamic: false,
        authorizers: Some(Arc::new(AuthorizerSet::new(vec![
            authorizer.clone() as Arc<dyn IDynamicAuthorizer>
        ]))),
    };

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    ctx.set_claims(Box::new(claims_without_permissions()));

    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());
    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 403, "static regression: claims still gate the route");
    let body_json: serde_json::Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert_eq!(
        body_json["required_permissions"],
        serde_json::json!(["settings:write"])
    );
    assert!(
        authorizer.seen().is_empty(),
        "the static gate short-circuits before dynamic authorization"
    );
}

#[tokio::test]
async fn dynamic_flag_without_rules_still_requires_authentication() {
    // Hand-written metadata can set the dynamic flag with an empty rule list;
    // the endpoint must still be protected (authenticated → delegated).
    let endpoint = StubEndpoint {
        method: "POST",
        path: "/orders",
        handler_type: "CreateOrderRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        authorization: &[],
        authorization_dynamic: true,
        authorizers: Some(Arc::new(AuthorizerSet::new(vec![Arc::new(
            AllowAllAuthorizer,
        )]))),
    };

    let mut anonymous = test_utils::TestHttpContext::new("POST", "/orders");
    let err = endpoint
        .handle(&mut anonymous)
        .await
        .expect_err("a dynamic endpoint without declared rules still requires authentication");
    assert!(matches!(&err, Error::Unauthorized(_)), "got {err:?}");

    let mut authenticated = test_utils::TestHttpContext::new("POST", "/orders");
    authenticated.set_claims(Box::new(claims_without_permissions()));
    endpoint
        .handle(&mut authenticated)
        .await
        .expect("authenticated principal is delegated to the authorizer");
    let (status, _, _) = authenticated.into_response_parts();
    assert_eq!(status, 200);
}

#[tokio::test]
async fn dynamic_endpoint_reaches_legacy_authorize_only_implementation() {
    // WEBX-19: the default `authorize_with_declared_rules` forwards through
    // `authorize_with_rules` to `authorize`, so a 1.1-style authorizer (only
    // `authorize`) still works on a dynamic endpoint without code changes.
    let endpoint = dynamic_endpoint(
        Arc::new(AuthorizerSet::new(vec![Arc::new(AllowAllAuthorizer)])),
        SETTINGS_WRITE_RULE,
    );

    let mut ctx = test_utils::TestHttpContext::new("POST", "/orders");
    *ctx.request_mut().route_pattern_mut() = Some("/orders".into());
    ctx.set_claims(Box::new(claims_without_permissions()));

    endpoint
        .handle(&mut ctx)
        .await
        .expect("legacy authorize() implementation keeps working through the default");
    let (status, _, _) = ctx.into_response_parts();
    assert_eq!(status, 200);
}

