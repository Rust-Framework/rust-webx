//! Request ID middleware —injects a unique `x-request-id` header into every response.

use std::ops::ControlFlow;
use uuid::Uuid;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

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

/// Whether an upstream correlation id is safe to echo and use as a trace id.
///
/// Request ids are attacker-controlled. Unbounded values can create huge
/// response headers, and arbitrary whitespace/control bytes belong nowhere in
/// audit correlation fields.
pub(crate) fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl RequestIdMiddleware {
    fn apply(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let id = ctx
            .request()
            .header("x-request-id")
            .filter(|value| is_valid_request_id(value))
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        ctx.response_mut().set_header_owned("x-request-id", id);
        Ok(ControlFlow::Continue(()))
    }
}

#[async_trait::async_trait]
impl IMiddleware for RequestIdMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        self.apply(ctx)
    }

    fn fast_invoke(&self, ctx: &mut dyn IHttpContext) -> Option<Result<ControlFlow<()>>> {
        Some(self.apply(ctx))
    }

    fn has_after(&self) -> bool {
        false
    }
}
