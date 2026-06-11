//! Routing traits: IRouter and IEndpoint.

use crate::error::Result;
use crate::http::IHttpContext;

/// HTTP methods supported by the framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "PUT" => Some(HttpMethod::Put),
            "DELETE" => Some(HttpMethod::Delete),
            "PATCH" => Some(HttpMethod::Patch),
            "HEAD" => Some(HttpMethod::Head),
            "OPTIONS" => Some(HttpMethod::Options),
            _ => None,
        }
    }
}

/// Route metadata: method + path pattern.
#[derive(Debug, Clone)]
pub struct RouteMeta {
    pub method: HttpMethod,
    pub path: String,
}

impl RouteMeta {
    pub fn new(method: HttpMethod, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
        }
    }
}

/// An endpoint handler is the terminal component in the middleware pipeline.
///
/// Analogous to ASP.NET Core's RequestDelegate at the endpoint level.
#[async_trait::async_trait]
pub trait IEndpoint: Send + Sync {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()>;
}

/// Router that matches incoming HTTP requests to registered endpoints.
///
/// Analogous to ASP.NET Core's IRouter / EndpointRouting.
#[async_trait::async_trait]
pub trait IRouter: Send + Sync {
    /// Register a new route with the router.
    fn register(&mut self, method: HttpMethod, path: &str, endpoint: Arc<dyn IEndpoint>);

    /// Match an incoming request to a registered endpoint.
    ///
    /// Returns a tuple of:
    /// - `Arc<dyn IEndpoint>` — the matched endpoint handler.
    /// - `HashMap<String, String>` — route parameter values.
    /// - `String` — the original route pattern (e.g., `"/api/users/{id}"`).
    async fn match_route(
        &self,
        ctx: &mut dyn IHttpContext,
    ) -> Result<
        Option<(
            Arc<dyn IEndpoint>,
            std::collections::HashMap<String, String>,
            String,
        )>,
    >;
}

use std::sync::Arc;
