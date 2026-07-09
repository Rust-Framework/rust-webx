//! Structured request tracing middleware with trace_id injection.

use rust_webx_core::error::Result;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static TRACE_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct RequestTracing {
    pub log_all: bool,
}

impl RequestTracing {
    pub fn new() -> Self {
        Self { log_all: true }
    }
    pub fn errors_only() -> Self {
        Self { log_all: false }
    }
}

impl Default for RequestTracing {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IMiddleware for RequestTracing {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // Propagate existing request ID from upstream, or generate a new one.
        let tid = ctx.request().header("x-request-id").map(|s| s.to_string())
            .unwrap_or_else(next_trace_id);
        ctx.response_mut().set_header("x-trace-id", &tid);
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let status = ctx.response().status();
        let is_err = status >= 400;

        if self.log_all || is_err {
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            let method = ctx.request().method().to_string();
            let path = ctx.request().path().to_string();
            let user = ctx.claims().map(|c| c.subject().to_string());

            if is_err {
                tracing::warn!(
                    count = count, method = %method, path = %path,
                    status = status, user = %user.as_deref().unwrap_or("anon"),
                    "request error"
                );
            } else {
                tracing::info!(
                    count = count, method = %method, path = %path,
                    status = status, user = %user.as_deref().unwrap_or("anon"),
                    "request ok"
                );
            }
        }
        Ok(())
    }
}

fn next_trace_id() -> String {
    let id = TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}", id)
}
