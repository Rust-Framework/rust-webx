//! Test utilities for the lrwf-http test suite.
//!
//! Provides a mock IHttpContext that can be used to test middleware,
//! routers, and other HTTP-layer components without spawning a hyper server.

#![allow(dead_code)]

use rust_webapp_core::auth::IClaims;
use rust_webapp_core::http::{IClaimsExt, IHttpContext, IHttpRequest, IHttpResponse};
use std::cell::RefCell;
use std::collections::HashMap;

/// A minimal mock IHttpContext for tests.
pub struct TestHttpContext {
    req: TestHttpRequest,
    resp: TestHttpResponse,
    claims: RefCell<Option<Box<dyn IClaims>>>,
}

impl TestHttpContext {
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            req: TestHttpRequest {
                method: method.to_string(),
                path: path.to_string(),
                headers: HashMap::new(),
                query_params: HashMap::new(),
                route_params: HashMap::new(),
                route_pattern: None,
                body_bytes: Vec::new(),
            },
            resp: TestHttpResponse {
                status: 200,
                headers: Vec::new(),
                body: None,
            },
            claims: RefCell::new(None),
        }
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.req.headers.insert(key.to_string(), value.to_string());
        self
    }

    pub fn with_body_json(mut self, json: &serde_json::Value) -> Self {
        self.req.body_bytes = serde_json::to_vec(json).unwrap();
        self
    }

    /// Consume the context and return response status + headers + body for assertions.
    pub fn into_response_parts(self) -> (u16, Vec<(String, String)>, Option<Vec<u8>>) {
        (self.resp.status, self.resp.headers, self.resp.body)
    }
}

impl IClaimsExt for TestHttpContext {
    fn set_claims(&mut self, claims: Box<dyn IClaims>) {
        *self.claims.borrow_mut() = Some(claims);
    }

    fn claims(&self) -> Option<&dyn IClaims> {
        let borrowed = self.claims.borrow();
        borrowed
            .as_ref()
            .map(|b| unsafe { &*(&**b as *const dyn IClaims) })
    }
}

impl IHttpContext for TestHttpContext {
    fn request(&self) -> &dyn IHttpRequest {
        &self.req
    }

    fn request_mut(&mut self) -> &mut dyn IHttpRequest {
        &mut self.req
    }

    fn response(&self) -> &dyn IHttpResponse {
        &self.resp
    }

    fn response_mut(&mut self) -> &mut dyn IHttpResponse {
        &mut self.resp
    }
}

// â”€â”€ TestHttpRequest â”€â”€

struct TestHttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    query_params: HashMap<String, String>,
    route_params: HashMap<String, String>,
    route_pattern: Option<String>,
    body_bytes: Vec<u8>,
}

#[async_trait::async_trait]
impl IHttpRequest for TestHttpRequest {
    fn method(&self) -> &str {
        &self.method
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|s| s.as_str())
    }

    fn query(&self) -> &HashMap<String, String> {
        &self.query_params
    }

    fn route_params(&self) -> &HashMap<String, String> {
        &self.route_params
    }

    fn route_params_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.route_params
    }

    fn route_pattern(&self) -> Option<&str> {
        self.route_pattern.as_deref()
    }

    fn route_pattern_mut(&mut self) -> &mut Option<String> {
        &mut self.route_pattern
    }

    async fn body_bytes(&self) -> rust_webapp_core::error::Result<Vec<u8>> {
        Ok(self.body_bytes.clone())
    }

    async fn body_text(&self) -> rust_webapp_core::error::Result<String> {
        String::from_utf8(self.body_bytes.clone())
            .map_err(|e| rust_webapp_core::error::Error::Http(e.to_string()))
    }
}

// â”€â”€ TestHttpResponse â”€â”€

struct TestHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[async_trait::async_trait]
impl IHttpResponse for TestHttpResponse {
    fn status(&self) -> u16 {
        self.status
    }

    fn has_body(&self) -> bool {
        self.body.is_some()
    }

    fn set_status(&mut self, code: u16) {
        self.status = code;
    }

    fn set_header(&mut self, key: &str, value: &str) {
        self.headers.push((key.to_string(), value.to_string()));
    }

    async fn write_bytes(&mut self, data: Vec<u8>) -> rust_webapp_core::error::Result<()> {
        self.body = Some(data);
        Ok(())
    }

    async fn write_text(&mut self, text: &str) -> rust_webapp_core::error::Result<()> {
        self.body = Some(text.as_bytes().to_vec());
        Ok(())
    }
}
