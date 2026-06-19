//! Middleware pipeline â€?Sequential execution model.
//!
//! Middlewares are called in registration order. Each middleware can
//! inspect or modify the request. The final handler (router) is called
//! after all middlewares have passed.

use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Boxed final handler function type.
pub type HandlerFn = Arc<
    dyn for<'a> Fn(
            &'a mut dyn IHttpContext,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>
        + Send
        + Sync,
>;

pub struct MiddlewarePipeline {
    middlewares: Vec<Arc<dyn IMiddleware>>,
}

impl MiddlewarePipeline {
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    pub fn add_middleware(&mut self, middleware: Arc<dyn IMiddleware>) {
        self.middlewares.push(middleware);
    }

    /// Execute middleware onion: invoke forward, final handler, after hooks reverse.
    pub async fn execute(
        &self,
        ctx: &mut dyn IHttpContext,
        final_handler: HandlerFn,
    ) -> Result<()> {
        // Forward pass: invoke each middleware
        for middleware in &self.middlewares {
            middleware.invoke(ctx).await?;
        }

        // Run the final handler (router)
        final_handler(ctx).await?;

        // Reverse pass: after hooks
        for middleware in self.middlewares.iter().rev() {
            middleware.after(ctx).await?;
        }

        Ok(())
    }
}

impl Default for MiddlewarePipeline {
    fn default() -> Self {
        Self::new()
    }
}
