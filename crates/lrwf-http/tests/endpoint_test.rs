//! Tests for StubEndpoint authorization checks and dispatch lifecycle.
//!
//! Uses the mock `TestHttpContext` from `test_utils.rs`.

mod test_utils;

use lrwf_core::auth::{IClaims, IDynamicAuthorizer};
use lrwf_core::di::scan::ResponseData;
use lrwf_core::error::{Error, Result as LrwfResult};
use lrwf_core::http::{IClaimsExt, IHttpContext};
use lrwf_core::routing::IEndpoint;
use lrwf_http::authz::AuthorizerSet;
use lrwf_http::endpoint::StubEndpoint;
use std::collections::HashMap;
use std::sync::Arc;

// ── Mock IClaims ─────────────────────────────────────────────────────

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

// ── Mock dispatch function ──────────────────────────────────────────

fn mock_dispatch_ok(
    _body: Vec<u8>,
    _route_params: HashMap<String, String>,
    _query_params: HashMap<String, String>,
    _claims: Option<Box<dyn IClaims>>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = LrwfResult<ResponseData>> + Send>> {
    Box::pin(async move {
        Ok(ResponseData {
            status: 200,
            content_type: "application/json".to_string(),
            body: serde_json::to_vec(&serde_json::json!({"ok":true})).unwrap(),
        })
    })
}

// ── Mock dynamic authorizers ────────────────────────────────────────

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

// ── Tests ───────────────────────────────────────────────────────────

#[tokio::test]
async fn endpoint_normal_dispatch_returns_200() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/test",
        handler_type: "TestRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "",
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
async fn endpoint_no_dispatch_fn_falls_back_to_stub() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/stub",
        handler_type: "MissingHandler",
        dispatch_fn: None,
        auth_required_role: "",
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/stub");
    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 200);
    let body_text = String::from_utf8(body.unwrap()).unwrap();
    assert!(body_text.contains("Matched route"));
    assert!(body_text.contains("/stub"));
}

#[tokio::test]
async fn endpoint_auth_required_with_valid_claims_succeeds() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "admin",
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
async fn endpoint_auth_required_without_claims_returns_401() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "admin",
        authorizers: None,
    };

    let mut ctx = test_utils::TestHttpContext::new("GET", "/admin");
    let result = endpoint.handle(&mut ctx).await;
    assert!(result.is_ok());

    let (status, _, body) = ctx.into_response_parts();
    assert_eq!(status, 401);
    let body_json: serde_json::Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert!(body_json["error"]
        .as_str()
        .unwrap()
        .contains("Authentication"));
}

#[tokio::test]
async fn endpoint_auth_required_wrong_role_returns_403() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/admin",
        handler_type: "AdminRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "admin",
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
    assert_eq!(body_json["required_role"], "admin");
}

#[tokio::test]
async fn endpoint_auth_with_authenticated_role_allows_any_valid_jwt() {
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/profile",
        handler_type: "ProfileRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "authenticated",
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
    let authorizers = Arc::new(AuthorizerSet::new(vec![
        Arc::new(DenyAllAuthorizer),
    ]));
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/secret",
        handler_type: "SecretRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "admin",
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
    let authorizers = Arc::new(AuthorizerSet::new(vec![
        Arc::new(AllowAllAuthorizer),
    ]));
    let endpoint = StubEndpoint {
        method: "GET",
        path: "/allowed",
        handler_type: "AllowedRequest",
        dispatch_fn: Some(mock_dispatch_ok),
        auth_required_role: "admin",
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
        auth_required_role: "admin",
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
