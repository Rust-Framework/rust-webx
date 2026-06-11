//! Security headers middleware for the LRWF framework.
//!
//! Adds recommended security-related HTTP response headers to every response.

use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;

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
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let resp = ctx.response_mut();

        // Prevent MIME type sniffing
        resp.set_header("x-content-type-options", "nosniff");

        // Prevent clickjacking
        resp.set_header("x-frame-options", "DENY");

        // Enable browser XSS filter
        resp.set_header("x-xss-protection", "1; mode=block");

        // Referrer policy
        resp.set_header("referrer-policy", "strict-origin-when-cross-origin");

        // Permissions policy (disable sensitive features)
        resp.set_header(
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        );

        // Cache control for API responses
        resp.set_header("cache-control", "no-store");

        Ok(())
    }
}
