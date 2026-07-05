//! Benchmark: Middleware pipeline execution.
//!
//! Measures pipeline throughput with varying numbers of middleware.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rust_webapp_core::error::Result as LrwfResult;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use rust_webapp_host::pipeline::{HandlerFn, MiddlewarePipeline};
use std::ops::ControlFlow;
use std::sync::Arc;

/// No-op middleware for benchmarking.
struct NoopMiddleware;

#[async_trait::async_trait]
impl IMiddleware for NoopMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<ControlFlow<()>> {
        Ok(ControlFlow::Continue(()))
    }
}

fn bench_pipeline_empty(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pipeline = MiddlewarePipeline::new();

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            Ok(())
        })
    });

    c.bench_function("pipeline_empty", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestContext::new();
                let result = pipeline.execute(&mut ctx, Arc::clone(&final_handler)).await;
                black_box(result).unwrap();
            })
        })
    });
}

fn bench_pipeline_3_middleware(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(Arc::new(NoopMiddleware));
    pipeline.add_middleware(Arc::new(NoopMiddleware));
    pipeline.add_middleware(Arc::new(NoopMiddleware));

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            Ok(())
        })
    });

    c.bench_function("pipeline_3_middleware", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestContext::new();
                let result = pipeline.execute(&mut ctx, Arc::clone(&final_handler)).await;
                black_box(result).unwrap();
            })
        })
    });
}

fn bench_pipeline_10_middleware(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut pipeline = MiddlewarePipeline::new();
    for _ in 0..10 {
        pipeline.add_middleware(Arc::new(NoopMiddleware));
    }

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            Ok(())
        })
    });

    c.bench_function("pipeline_10_middleware", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut ctx = TestContext::new();
                let result = pipeline.execute(&mut ctx, Arc::clone(&final_handler)).await;
                black_box(result).unwrap();
            })
        })
    });
}

// â”€â”€ Minimal context â”€â”€

use std::collections::HashMap;

struct TestReq {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    query_params: HashMap<String, String>,
    route_params: HashMap<String, String>,
    route_pattern: Option<String>,
    body_bytes: Vec<u8>,
}

#[async_trait::async_trait]
impl rust_webapp_core::http::IHttpRequest for TestReq {
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

struct TestResp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

#[async_trait::async_trait]
impl rust_webapp_core::http::IHttpResponse for TestResp {
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

struct TestContext {
    req: TestReq,
    resp: TestResp,
    claims: std::cell::RefCell<Option<Box<dyn rust_webapp_core::auth::IClaims>>>,
}

impl TestContext {
    fn new() -> Self {
        Self {
            req: TestReq {
                method: "GET".into(),
                path: "/".into(),
                headers: HashMap::new(),
                query_params: HashMap::new(),
                route_params: HashMap::new(),
                route_pattern: None,
                body_bytes: Vec::new(),
            },
            resp: TestResp {
                status: 200,
                headers: Vec::new(),
                body: None,
            },
            claims: std::cell::RefCell::new(None),
        }
    }
}

impl rust_webapp_core::http::IClaimsExt for TestContext {
    fn set_claims(&mut self, c: Box<dyn rust_webapp_core::auth::IClaims>) {
        *self.claims.borrow_mut() = Some(c);
    }
    fn claims(&self) -> Option<&dyn rust_webapp_core::auth::IClaims> {
        let b = self.claims.borrow();
        b.as_ref()
            .map(|c| unsafe { &*(&**c as *const dyn rust_webapp_core::auth::IClaims) })
    }
}

impl IHttpContext for TestContext {
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
    bench_pipeline_empty,
    bench_pipeline_3_middleware,
    bench_pipeline_10_middleware
);
criterion_main!(benches);
