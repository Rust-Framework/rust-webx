//! Cross-Origin Resource Sharing (CORS) middleware.
//!
//! Automatically configured from `appsettings.json` `Cors` section.
//! Handles preflight OPTIONS requests and sets CORS response headers.

use std::ops::ControlFlow;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

/// CORS configuration loaded from appsettings.json.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    /// Allowed origins. Use `"*"` to allow any origin.
    pub origins: Vec<String>,
    /// Allowed HTTP methods (default: GET, POST, PUT, DELETE, OPTIONS).
    pub methods: Vec<String>,
    /// Allowed request headers.
    pub headers: Vec<String>,
    /// Response headers exposed to the client (via access-control-expose-headers).
    pub expose_headers: Vec<String>,
    /// Whether credentials (cookies) are allowed.
    pub allow_credentials: bool,
    /// Max age for preflight cache (seconds).
    pub max_age: u32,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            // CORS is opt-in: an empty origin list sends no CORS headers.
            origins: Vec::new(),
            methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "PATCH".to_string(),
                "OPTIONS".to_string(),
            ],
            headers: vec!["Content-Type".to_string(), "Authorization".to_string()],
            expose_headers: Vec::new(),
            allow_credentials: false,
            max_age: 86400,
        }
    }
}

/// Built-in CORS middleware.
///
/// Reads config from `appsettings.json` →`Cors` section at build time
/// and applies CORS headers to every response. Handles preflight OPTIONS.
pub struct CorsMiddleware {
    config: CorsConfig,
}

impl CorsMiddleware {
    /// Create a new CORS middleware with the given configuration.
    pub fn new(config: CorsConfig) -> Self {
        Self { config }
    }
}

impl CorsMiddleware {
    fn apply(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // Owned, because the response is mutated below while the Origin value
        // is still needed for the preflight decision.
        let Some(origin) = ctx.request().header("origin").map(str::to_string) else {
            // 2.4-9 fast path: no Origin means no CORS processing for this
            // response. Skipping the `Vary: Origin` append is safe because a
            // response without CORS headers has nothing to vary by.
            return Ok(ControlFlow::Continue(()));
        };

        // A response that may change with Origin must say so, otherwise a
        // shared cache can serve one origin's CORS headers to another.
        if !self.config.origins.is_empty() {
            ctx.response_mut().append_header("vary", "Origin");
        }

        // Determine allowed origin
        let allowed = self
            .config
            .origins
            .iter()
            .find(|o| o.as_str() == "*" || **o == origin)
            .cloned();

        if let Some(ref origin_value) = allowed {
            // When allow_credentials is true, "*" cannot be used per CORS spec;
            // echo the request's Origin header instead. A specific configured
            // origin is echoed too (it matched the request Origin above).
            let header_value = if origin_value == "*" && !self.config.allow_credentials {
                "*".to_string()
            } else {
                origin.clone()
            };

            ctx.response_mut()
                .set_header("access-control-allow-origin", &header_value);

            if self.config.allow_credentials {
                ctx.response_mut()
                    .set_header("access-control-allow-credentials", "true");
            }
        }

        // Only a real CORS preflight is short-circuited here: it must be an
        // OPTIONS request that carries both `Origin` and
        // `Access-Control-Request-Method`. A plain `OPTIONS` (curl, method
        // discovery) belongs to the router, which answers with `Allow` or 404 —
        // answering 204 for any OPTIONS made nonexistent paths look like
        // existing ones.
        let is_preflight = ctx.request().method().eq_ignore_ascii_case("OPTIONS")
            && ctx
                .request()
                .header("access-control-request-method")
                .is_some();
        if is_preflight && allowed.is_some() {
            ctx.response_mut().set_status(204);
            ctx.response_mut().set_header(
                "access-control-allow-methods",
                &self.config.methods.join(", "),
            );
            ctx.response_mut().set_header(
                "access-control-allow-headers",
                &self.config.headers.join(", "),
            );
            ctx.response_mut()
                .set_header("access-control-max-age", &self.config.max_age.to_string());
            // Short-circuit: preflight response is complete, no need to
            // continue to router or final handler.
            return Ok(ControlFlow::Break(()));
        }

        // For normal requests, expose specified response headers to the client
        if !self.config.expose_headers.is_empty() {
            ctx.response_mut().set_header(
                "access-control-expose-headers",
                &self.config.expose_headers.join(", "),
            );
        }

        Ok(ControlFlow::Continue(()))
    }
}

#[async_trait::async_trait]
impl IMiddleware for CorsMiddleware {
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
