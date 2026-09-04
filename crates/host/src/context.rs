//! HttpContext —Runtime implementation of IHttpContext, IHttpRequest, IHttpResponse.

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::Request;
use rust_webx_core::auth::IClaims;
use rust_webx_core::error::Result;
use rust_webx_core::http::{IClaimsExt, IHttpContext, IHttpRequest, IHttpResponse};

use crate::problem_response::{build_problem, problem_to_bytes};
use std::collections::HashMap;

/// Concrete implementation of IHttpContext wrapping a hyper request and response.
///
/// The request body is eagerly read during construction so that
/// the IHttpRequest trait can provide sync access to body bytes.
pub struct HttpContext {
    req: HttpRequest,
    resp: HttpResponse,
    /// Authentication claims set by authentication middleware.
    claims: Option<Box<dyn IClaims>>,
}

impl HttpContext {
    /// Create a new HttpContext by eagerly reading the request body.
    ///
    /// Reads the body in chunks using [`BodyExt::frame`], tracking total size.
    /// When `max_body_size` is exceeded, the context is built with a 413
    /// Payload Too Large response pre-set.
    pub async fn new(req: Request<Incoming>, max_body_size: usize) -> Self {
        use http_body_util::BodyExt;

        let (parts, mut body) = req.into_parts();
        let method = parts.method.to_string();
        let path = parts.uri.path().to_string();

        // Read headers
        let mut headers = HashMap::new();
        for (name, value) in parts.headers.iter() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.to_string(), v.to_string());
            }
        }

        // Parse query string
        let query_params = parts
            .uri
            .query()
            .map(parse_query_string)
            .unwrap_or_default();

        // Read body in chunks, tracking total size
        let mut body_bytes = Vec::new();
        let mut total_size: usize = 0;
        let mut payload_too_large = false;

        while let Some(frame_result) = body.frame().await {
            match frame_result {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        total_size = total_size.saturating_add(data.len());
                        if total_size > max_body_size {
                            payload_too_large = true;
                            break;
                        }
                        body_bytes.extend_from_slice(data);
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }

        let req = HttpRequest {
            method,
            path,
            headers,
            query_params,
            route_params: HashMap::new(),
            route_pattern: None,
            body_bytes,
        };

        let mut resp = HttpResponse::new(200);

        if payload_too_large {
            resp.set_status(413);
            let problem = build_problem(413, "Request body exceeds maximum allowed size");
            resp.body = Some(problem_to_bytes(&problem));
            resp.headers.insert(
                "content-type".to_string(),
                "application/problem+json".to_string(),
            );
        }

        Self {
            req,
            resp,
            claims: None,
        }
    }

    pub fn into_response(self) -> hyper::Response<Full<Bytes>> {
        self.resp.into_hyper()
    }
}

impl IClaimsExt for HttpContext {
    fn set_claims(&mut self, claims: Box<dyn IClaims>) {
        self.claims = Some(claims);
    }

    fn claims(&self) -> Option<&dyn IClaims> {
        self.claims.as_deref()
    }
}

impl IHttpContext for HttpContext {
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

/// Concrete implementation of IHttpRequest.
///
/// Body bytes are stored eagerly so that IHttpRequest can be dyn-compatible
/// without requiring `&mut self` for async body reading.
pub struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    query_params: HashMap<String, String>,
    route_params: HashMap<String, String>,
    route_pattern: Option<String>,
    body_bytes: Vec<u8>,
}

#[async_trait::async_trait]
impl IHttpRequest for HttpRequest {
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

    async fn body_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.body_bytes.clone())
    }

    async fn body_text(&self) -> Result<String> {
        String::from_utf8(self.body_bytes.clone())
            .map_err(|e| rust_webx_core::error::Error::Http(e.to_string()))
    }
}

/// Concrete implementation of IHttpResponse.
pub struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Option<Vec<u8>>,
}

impl HttpResponse {
    pub fn new(status: u16) -> Self {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        Self {
            status,
            headers,
            body: None,
        }
    }

    pub fn into_hyper(self) -> hyper::Response<Full<Bytes>> {
        let mut builder = hyper::Response::builder().status(self.status);
        for (key, value) in &self.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let body_bytes = self.body.unwrap_or_default();
        builder
            .body(Full::new(Bytes::from(body_bytes)))
            .unwrap_or_else(|_| {
                hyper::Response::builder()
                    .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Full::new(Bytes::from("Internal Server Error")))
                    .unwrap()
            })
    }
}

#[async_trait::async_trait]
impl IHttpResponse for HttpResponse {
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
        self.headers.insert(key.to_string(), value.to_string());
    }

    fn remove_header(&mut self, key: &str) {
        self.headers.remove(key);
    }

    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()> {
        self.body = Some(data);
        Ok(())
    }

    async fn write_text(&mut self, text: &str) -> Result<()> {
        self.body = Some(text.as_bytes().to_vec());
        Ok(())
    }

    fn body_bytes(&self) -> Vec<u8> {
        self.body.clone().unwrap_or_default()
    }

    fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(|s| s.as_str())
    }
}

/// Parse a query string like "key1=val1&key2=val2" into a HashMap.
fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            params.insert(percent_decode(key), percent_decode(value));
        }
    }
    params
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
