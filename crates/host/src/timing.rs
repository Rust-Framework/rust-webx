//! Timing middleware —demonstrates the after hook by injecting a per-request counter.

use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST_COUNT: AtomicU64 = AtomicU64::new(0);

/// Records a request counter via the `after` hook.
///
/// The counter increments after the final handler has executed and is
/// injected as the `x-request-count` response header.
pub struct TimingMiddleware;

impl TimingMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TimingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IMiddleware for TimingMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let count = REQUEST_COUNT.fetch_add(1, Ordering::Relaxed);
        ctx.response_mut()
            .set_header("x-request-count", &count.to_string());
        Ok(())
    }
}
