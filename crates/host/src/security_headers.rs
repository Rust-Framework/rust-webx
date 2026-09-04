//! Security headers middleware for the rust-webx framework.
//!
//! Adds recommended security-related HTTP response headers to every response.

use rust_webx_core::error::Result;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::middleware::IMiddleware;
use std::ops::ControlFlow;

/// Middleware that adds a standard set of security headers to every response.
pub struct SecurityHeadersMiddleware;

impl SecurityHeadersMiddleware {
    /// Create a new security headers middleware.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SecurityHeadersMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IMiddleware for SecurityHeadersMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let resp = ctx.response_mut();

        // Prevent MIME type sniffing
        resp.set_header("x-content-type-options", "nosniff");

        // Prevent clickjacking
        resp.set_header("x-frame-options", "DENY");

        // Force HTTPS for 1 year (HSTS)
        resp.set_header(
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        );

        // Referrer policy
        resp.set_header("referrer-policy", "strict-origin-when-cross-origin");

        // Permissions policy (disable sensitive features)
        resp.set_header(
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        );

        // Cache control for API responses (overridable by static-file middleware)
        resp.set_header("cache-control", "no-store");

        Ok(ControlFlow::Continue(()))
    }
}
