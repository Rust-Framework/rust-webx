//! Request ID middleware —injects a unique `x-request-id` header into every response.

use rust_webx_core::error::Result;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use std::ops::ControlFlow;
use uuid::Uuid;

/// Generates a UUID v4 request ID on each request and injects it into the response.
pub struct RequestIdMiddleware;

impl RequestIdMiddleware {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RequestIdMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IMiddleware for RequestIdMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let id = ctx
            .request()
            .header("x-request-id")
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        ctx.response_mut().set_header("x-request-id", &id);
        Ok(ControlFlow::Continue(()))
    }
}
