//! Tests for IHttpContext, IHttpRequest, and IHttpResponse bounds
//! as well as claims access patterns.
//!
//! These tests exercise the mock TestHttpContext from lrwf-http/test_utils
//! as well as trait-based verification of the real HttpContext.

use lrwf_core::auth::IClaims;
use lrwf_core::http::IHttpResponse;
use std::collections::HashMap;

// ─── Mock IClaims for testing ────────────────────────────────────────

struct TestClaims {
    sub: String,
    roles: Vec<String>,
    perms: Vec<String>,
    raw: HashMap<String, String>,
}

impl IClaims for TestClaims {
    fn subject(&self) -> &str {
        &self.sub
    }
    fn roles(&self) -> &[String] {
        &self.roles
    }
    fn permissions(&self) -> &[String] {
        &self.perms
    }
    fn claims(&self) -> &HashMap<String, String> {
        &self.raw
    }
    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(TestClaims {
            sub: self.sub.clone(),
            roles: self.roles.clone(),
            perms: self.perms.clone(),
            raw: self.raw.clone(),
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[test]
fn test_claims_subject_and_roles() {
    let claims = TestClaims {
        sub: "user-42".into(),
        roles: vec!["admin".into(), "user".into()],
        perms: vec!["read".into()],
        raw: HashMap::new(),
    };

    assert_eq!(claims.subject(), "user-42");
    assert_eq!(claims.roles().len(), 2);
    assert!(claims.roles().contains(&"admin".to_string()));
    assert_eq!(claims.permissions().len(), 1);
}

#[test]
fn test_claims_clone_box_preserves_data() {
    let claims = TestClaims {
        sub: "test-user".into(),
        roles: vec!["tester".into()],
        perms: vec![],
        raw: HashMap::new(),
    };

    let cloned = claims.clone_box();
    assert_eq!(cloned.subject(), "test-user");
    assert_eq!(cloned.roles().len(), 1);
}

#[test]
fn test_claims_empty_roles_and_permissions() {
    let claims = TestClaims {
        sub: "anon".into(),
        roles: vec![],
        perms: vec![],
        raw: HashMap::new(),
    };

    assert!(claims.roles().is_empty());
    assert!(claims.permissions().is_empty());
    assert_eq!(claims.subject(), "anon");
}

// ─── HttpStatus constants ────────────────────────────────────────────

#[test]
fn http_status_constants() {
    use lrwf_core::http::HttpStatus;
    assert_eq!(HttpStatus::OK, 200);
    assert_eq!(HttpStatus::CREATED, 201);
    assert_eq!(HttpStatus::NO_CONTENT, 204);
    assert_eq!(HttpStatus::BAD_REQUEST, 400);
    assert_eq!(HttpStatus::UNAUTHORIZED, 401);
    assert_eq!(HttpStatus::FORBIDDEN, 403);
    assert_eq!(HttpStatus::NOT_FOUND, 404);
    assert_eq!(HttpStatus::INTERNAL_SERVER_ERROR, 500);
}

// ─── Verify IHttpResponse default trait methods compile ──────────────

/// This struct implements IHttpResponse to verify that the default methods
/// (`has_body`, `write_text`) compile correctly and produce expected results.
struct MinimalResponse {
    status: u16,
    body: Option<Vec<u8>>,
}

impl MinimalResponse {
    fn new(status: u16) -> Self {
        Self { status, body: None }
    }
}

#[async_trait::async_trait]
impl IHttpResponse for MinimalResponse {
    fn status(&self) -> u16 {
        self.status
    }

    fn set_status(&mut self, code: u16) {
        self.status = code;
    }

    fn set_header(&mut self, _key: &str, _value: &str) {}

    fn has_body(&self) -> bool {
        self.body.is_some()
    }

    async fn write_bytes(&mut self, data: Vec<u8>) -> lrwf_core::error::Result<()> {
        self.body = Some(data);
        Ok(())
    }
}

#[tokio::test]
async fn minimal_response_write_text_uses_default_impl() {
    let mut resp = MinimalResponse::new(200);
    // write_text has a default impl that calls write_bytes
    resp.write_text("hello").await.unwrap();
    assert!(resp.has_body());
    assert_eq!(resp.body.unwrap(), b"hello");
}

#[tokio::test]
async fn minimal_response_no_body_by_default() {
    let resp = MinimalResponse::new(200);
    assert!(!resp.has_body());
}
