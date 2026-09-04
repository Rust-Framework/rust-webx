//! HTTP request counters exposed at `GET /metrics` (Prometheus text format).

use rust_webx_core::error::Result;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use rust_webx_core::routing::IEndpoint;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Shared HTTP metrics counters.
#[derive(Default)]
pub struct HttpMetrics {
    total: AtomicU64,
    status_2xx: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
}

impl HttpMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, status: u16) {
        self.total.fetch_add(1, Ordering::Relaxed);
        match status {
            200..=299 => {
                self.status_2xx.fetch_add(1, Ordering::Relaxed);
            }
            400..=499 => {
                self.status_4xx.fetch_add(1, Ordering::Relaxed);
            }
            500..=599 => {
                self.status_5xx.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    pub fn render_prometheus(&self) -> String {
        let total = self.total.load(Ordering::Relaxed);
        let s2 = self.status_2xx.load(Ordering::Relaxed);
        let s4 = self.status_4xx.load(Ordering::Relaxed);
        let s5 = self.status_5xx.load(Ordering::Relaxed);
        format!(
            "# HELP http_requests_total Total HTTP requests processed\n\
             # TYPE http_requests_total counter\n\
             http_requests_total {total}\n\
             # HELP http_responses_2xx_total 2xx responses\n\
             # TYPE http_responses_2xx_total counter\n\
             http_responses_2xx_total {s2}\n\
             # HELP http_responses_4xx_total 4xx responses\n\
             # TYPE http_responses_4xx_total counter\n\
             http_responses_4xx_total {s4}\n\
             # HELP http_responses_5xx_total 5xx responses\n\
             # TYPE http_responses_5xx_total counter\n\
             http_responses_5xx_total {s5}\n"
        )
    }
}

/// Records response status codes into [`HttpMetrics`].
pub struct MetricsMiddleware {
    metrics: Arc<HttpMetrics>,
}

impl MetricsMiddleware {
    pub fn new(metrics: Arc<HttpMetrics>) -> Self {
        Self { metrics }
    }
}

#[async_trait::async_trait]
impl IMiddleware for MetricsMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<std::ops::ControlFlow<()>> {
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        self.metrics.record(ctx.response().status());
        Ok(())
    }
}

/// Serves Prometheus text metrics from a shared [`HttpMetrics`] instance.
pub struct MetricsEndpoint {
    metrics: Arc<HttpMetrics>,
}

impl MetricsEndpoint {
    pub fn new(metrics: Arc<HttpMetrics>) -> Self {
        Self { metrics }
    }
}

#[async_trait::async_trait]
impl IEndpoint for MetricsEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let body = self.metrics.render_prometheus();
        ctx.response_mut().set_status(200);
        ctx.response_mut()
            .set_header("content-type", "text/plain; version=0.0.4; charset=utf-8");
        ctx.response_mut().write_bytes(body.into_bytes()).await
    }
}
