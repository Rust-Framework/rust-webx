//! Endpoint — IEndpoint implementations for dual-mode dispatch.

use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::routing::IEndpoint;

/// Endpoint that wraps a boxed async handler.
pub struct RequestEndpoint {
    handler: Box<
        dyn Fn(
                &mut dyn IHttpContext,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>>
            + Send
            + Sync,
    >,
}

#[async_trait::async_trait]
impl IEndpoint for RequestEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        (self.handler)(ctx).await
    }
}

impl RequestEndpoint {
    pub fn new<F>(f: F) -> Self
    where
        F: for<'a> Fn(
                &'a mut dyn IHttpContext,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            handler: Box::new(f),
        }
    }
}

/// A no-op endpoint that responds with a descriptive message.
/// Used as a placeholder when routes are auto-registered from inventory.
pub struct StubEndpoint {
    pub method: &'static str,
    pub path: &'static str,
    pub handler_type: &'static str,
}

#[async_trait::async_trait]
impl IEndpoint for StubEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        ctx.response_mut().set_status(200);
        ctx.response_mut()
            .write_text(&format!(
                "Matched route: {} {} (handler: {})",
                self.method, self.path, self.handler_type
            ))
            .await?;
        Ok(())
    }
}

/// Endpoint for controller-based methods.
pub struct ControllerEndpoint {
    handler: Box<
        dyn Fn(
                &mut dyn IHttpContext,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>>
            + Send
            + Sync,
    >,
}

#[async_trait::async_trait]
impl IEndpoint for ControllerEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        (self.handler)(ctx).await
    }
}

impl ControllerEndpoint {
    pub fn new<F>(f: F) -> Self
    where
        F: for<'a> Fn(
                &'a mut dyn IHttpContext,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            handler: Box::new(f),
        }
    }
}

/// Endpoint that serves a static JSON payload.
///
/// Used for built-in endpoints like `/openapi.json`.
pub struct StaticJsonEndpoint {
    pub body: Vec<u8>,
}

#[async_trait::async_trait]
impl IEndpoint for StaticJsonEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        ctx.response_mut().set_status(200);
        ctx.response_mut()
            .set_header("content-type", "application/json");
        ctx.response_mut().write_bytes(self.body.clone()).await
    }
}

/// Endpoint that serves a static HTML payload.
///
/// Used for built-in endpoints like `/api/docs`.
pub struct StaticHtmlEndpoint {
    pub body: &'static str,
}

#[async_trait::async_trait]
impl IEndpoint for StaticHtmlEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        ctx.response_mut().set_status(200);
        ctx.response_mut()
            .set_header("content-type", "text/html; charset=utf-8");
        ctx.response_mut()
            .write_bytes(self.body.as_bytes().to_vec())
            .await
    }
}
