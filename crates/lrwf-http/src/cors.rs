//! Cross-Origin Resource Sharing (CORS) middleware.
//!
//! Automatically configured from `appsettings.json` `Cors` section.
//! Handles preflight OPTIONS requests and sets CORS response headers.

use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;
use std::sync::Arc;

/// CORS configuration loaded from appsettings.json.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    /// Allowed origins. Use `"*"` to allow any origin.
    pub origins: Vec<String>,
    /// Allowed HTTP methods (default: GET, POST, PUT, DELETE, OPTIONS).
    pub methods: Vec<String>,
    /// Allowed request headers.
    pub headers: Vec<String>,
    /// Whether credentials (cookies) are allowed.
    pub allow_credentials: bool,
    /// Max age for preflight cache (seconds).
    pub max_age: u32,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            origins: vec!["*".to_string()],
            methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "PATCH".to_string(),
                "OPTIONS".to_string(),
            ],
            headers: vec![
                "Content-Type".to_string(),
                "Authorization".to_string(),
            ],
            allow_credentials: false,
            max_age: 86400,
        }
    }
}

/// Built-in CORS middleware.
///
/// Reads config from `appsettings.json` → `Cors` section at build time
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

#[async_trait::async_trait]
impl IMiddleware for CorsMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let origin = ctx.request().header("origin");

        // Determine allowed origin
        let allowed = self
            .config
            .origins
            .iter()
            .find(|o| *o == "*" || origin.map_or(false, |orig| *o == orig))
            .cloned();

        if let Some(ref origin_value) = allowed {
            let header_value = if origin_value == "*" {
                "*".to_string()
            } else if let Some(o) = origin {
                o.to_string()
            } else {
                origin_value.clone()
            };

            ctx.response_mut()
                .set_header("access-control-allow-origin", &header_value);

            if self.config.allow_credentials {
                ctx.response_mut()
                    .set_header("access-control-allow-credentials", "true");
            }
        }

        // Handle preflight OPTIONS
        if ctx.request().method().to_uppercase() == "OPTIONS" {
            ctx.response_mut().set_status(204);
            ctx.response_mut().set_header(
                "access-control-allow-methods",
                &self.config.methods.join(", "),
            );
            ctx.response_mut().set_header(
                "access-control-allow-headers",
                &self.config.headers.join(", "),
            );
            ctx.response_mut().set_header(
                "access-control-max-age",
                &self.config.max_age.to_string(),
            );
        } else {
            // For normal requests, expose headers
            if let Some(ref expose) = allowed {
                if expose != "*" {
                    ctx.response_mut().set_header(
                        "access-control-expose-headers",
                        &self.config.headers.join(", "),
                    );
                }
            }
        }

        Ok(())
    }
}
