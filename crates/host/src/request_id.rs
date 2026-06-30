//! Request ID middleware —injects a unique `x-request-id` header into every response.

use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
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
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        ctx.response_mut().set_header("x-request-id", &id);
        Ok(())
    }
}
