//! Middleware trait: IMiddleware.

use crate::error::Result;
use crate::http::IHttpContext;

/// Middleware component in the HTTP request pipeline.
///
/// Middlewares are called in registration order. Each middleware can:
/// - Inspect/modify the request before passing through
/// - Short-circuit by calling methods on the response without returning
/// - The pipeline continues to the next middleware unless the response
///   status has been set (short-circuit detection).
///
/// Analogous to ASP.NET Core's IMiddleware.
#[async_trait::async_trait]
pub trait IMiddleware: Send + Sync {
    /// Process an HTTP request (before hook).
    ///
    /// Return `Ok(())` to continue the pipeline, or set the response
    /// status to short-circuit further processing.
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()>;

    /// Called after the final handler has executed.
    ///
    /// Middleware post-processing, logging, response modification.
    /// Default: no-op.
    async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> {
        Ok(())
    }
}
