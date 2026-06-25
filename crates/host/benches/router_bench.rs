//! Benchmark: Trie-based router matching.
//!
//! Measures static route, dynamic parameter, and 404 path performance.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::routing::{HttpMethod, IEndpoint, IRouter};
use rust_webapp_host::router::Router;
use std::sync::Arc;

/// Minimal endpoint for benchmarks.
struct BenchEndpoint;

#[async_trait::async_trait]
impl IEndpoint for BenchEndpoint {
    async fn handle(&self, _ctx: &mut dyn IHttpContext) -> rust_webapp_core::error::Result<()> {
        Ok(())
    }
}

fn build_benchmark_router() -> Router {
    let mut router = Router::new();
    let ep: Arc<dyn IEndpoint> = Arc::new(BenchEndpoint);

    // Static routes
    for i in 0..20 {
        router.register(
            HttpMethod::Get,
            &format!("/api/static/endpoint_{}", i),
            Arc::clone(&ep),
        );
    }

    // Dynamic parameter routes
    router.register(HttpMethod::Get, "/api/users/{id}", Arc::clone(&ep));
    router.register(
        HttpMethod::Get,
        "/api/users/{id}/posts/{post_id}",
        Arc::clone(&ep),
    );
    router.register(
        HttpMethod::Get,
        "/api/orgs/{org}/teams/{team}/members/{member}",
        Arc::clone(&ep),
    );

    // Nested routes
    router.register(HttpMethod::Get, "/api/deep/a/b/c/d/e", Arc::clone(&ep));

    router
}

fn bench_router_static_match(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let router = build_benchmark_router();

    c.bench_function("router_static_match", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestHttpContext::new("GET", "/api/static/endpoint_10");
                let result = router.match_route(&mut ctx).await.unwrap();
                black_box(result);
            })
        })
    });
}

fn bench_router_dynamic_param(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let router = build_benchmark_router();

    c.bench_function("router_dynamic_param_single", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestHttpContext::new("GET", "/api/users/42");
                let result = router.match_route(&mut ctx).await.unwrap();
                black_box(result);
            })
        })
    });
}

fn bench_router_dynamic_multi_param(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let router = build_benchmark_router();

    c.bench_function("router_dynamic_param_multi", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestHttpContext::new("GET", "/api/users/42/posts/99");
                let result = router.match_route(&mut ctx).await.unwrap();
                black_box(result);
            })
        })
    });
}

fn bench_router_404(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let router = build_benchmark_router();

    c.bench_function("router_404_not_found", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestHttpContext::new("GET", "/api/nonexistent/very/deep/path");
                let result = router.match_route(&mut ctx).await.unwrap();
                black_box(result);
            })
        })
    });
}

fn bench_router_deep_nested(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let router = build_benchmark_router();

    c.bench_function("router_deep_nested", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestHttpContext::new("GET", "/api/deep/a/b/c/d/e");
                let result = router.match_route(&mut ctx).await.unwrap();
                black_box(result);
            })
        })
    });
}

fn bench_router_method_mismatch(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let router = build_benchmark_router();

    c.bench_function("router_method_mismatch", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestHttpContext::new("POST", "/api/static/endpoint_10");
                let result = router.match_route(&mut ctx).await.unwrap();
                black_box(result);
            })
        })
    });
}

// â”€â”€ Minimal test context â”€â”€

use std::collections::HashMap;

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
impl rust_webapp_core::http::IHttpRequest for TestHttpRequest {
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

struct TestHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[async_trait::async_trait]
impl rust_webapp_core::http::IHttpResponse for TestHttpResponse {
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

struct TestHttpContext {
    req: TestHttpRequest,
    resp: TestHttpResponse,
    claims: std::cell::RefCell<Option<Box<dyn rust_webapp_core::auth::IClaims>>>,
}

impl TestHttpContext {
    fn new(method: &str, path: &str) -> Self {
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
            claims: std::cell::RefCell::new(None),
        }
    }
}

impl rust_webapp_core::http::IClaimsExt for TestHttpContext {
    fn set_claims(&mut self, claims: Box<dyn rust_webapp_core::auth::IClaims>) {
        *self.claims.borrow_mut() = Some(claims);
    }
    fn claims(&self) -> Option<&dyn rust_webapp_core::auth::IClaims> {
        let borrowed = self.claims.borrow();
        borrowed
            .as_ref()
            .map(|b| unsafe { &*(&**b as *const dyn rust_webapp_core::auth::IClaims) })
    }
}

impl IHttpContext for TestHttpContext {
    fn request(&self) -> &dyn rust_webapp_core::http::IHttpRequest {
        &self.req
    }
    fn request_mut(&mut self) -> &mut dyn rust_webapp_core::http::IHttpRequest {
        &mut self.req
    }
    fn response(&self) -> &dyn rust_webapp_core::http::IHttpResponse {
        &self.resp
    }
    fn response_mut(&mut self) -> &mut dyn rust_webapp_core::http::IHttpResponse {
        &mut self.resp
    }
}

criterion_group!(
    benches,
    bench_router_static_match,
    bench_router_dynamic_param,
    bench_router_dynamic_multi_param,
    bench_router_404,
    bench_router_deep_nested,
    bench_router_method_mismatch,
);
criterion_main!(benches);
