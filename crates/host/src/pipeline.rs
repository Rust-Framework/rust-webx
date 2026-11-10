//! Middleware pipeline —Sequential execution model.
//!
//! Middlewares are called in registration order. Each middleware can
//! inspect or modify the request. The final handler (router) is called
//! after all middlewares have passed.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;

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
    ///
    /// Supports short-circuit: if a middleware's `invoke` returns
    /// `ControlFlow::Break(())`, remaining middleware and the final handler are
    /// skipped.
    ///
    /// `after` hooks always run for the middleware whose `invoke` completed,
    /// including when the request fails: an error raised by a middleware or by
    /// the final handler must still let metrics, tracing, gzip and cleanup
    /// observe the response. The first error is returned after the unwind; a
    /// failure inside an `after` hook is logged and never masks the original
    /// error.
    pub async fn execute(
        &self,
        ctx: &mut dyn IHttpContext,
        final_handler: HandlerFn,
    ) -> Result<()> {
        use std::ops::ControlFlow;

        let mut executed: Vec<Arc<dyn IMiddleware>> = Vec::new();
        let mut short_circuited = false;
        let mut failure: Option<Error> = None;

        // Forward pass: invoke each middleware, track executed for after hooks
        for middleware in &self.middlewares {
            match middleware.invoke(ctx).await {
                Ok(ControlFlow::Continue(())) => executed.push(Arc::clone(middleware)),
                Ok(ControlFlow::Break(())) => {
                    short_circuited = true;
                    break;
                }
                Err(err) => {
                    failure = Some(err);
                    break;
                }
            }
        }

        // Run the final handler (router) only if not short-circuited and the
        // forward pass did not already fail.
        if !short_circuited && failure.is_none() {
            if let Err(err) = final_handler(ctx).await {
                failure = Some(err);
            }
        }

        // Reverse pass: after hooks on executed middleware only, error path
        // included.
        for middleware in executed.into_iter().rev() {
            if let Err(err) = middleware.after(ctx).await {
                if failure.is_none() {
                    failure = Some(err);
                } else {
                    tracing::error!(error = %err, "middleware after hook failed");
                }
            }
        }

        match failure {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

impl Default for MiddlewarePipeline {
    fn default() -> Self {
        Self::new()
    }
}
