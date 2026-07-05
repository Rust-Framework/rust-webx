//! Middleware trait: IMiddleware.

use crate::error::Result;
use crate::http::IHttpContext;
use std::ops::ControlFlow;

/// Middleware component in the HTTP request pipeline.
///
/// Middlewares are called in registration order. Each middleware can:
/// - Inspect/modify the request before passing through
/// - Short-circuit by returning `ControlFlow::Break(())`
/// - The pipeline continues to the next middleware on `ControlFlow::Continue(())`
///
/// On short-circuit, remaining middleware and the final handler are skipped,
/// but `after` hooks on already-executed middleware still run in reverse order.
///
/// Analogous to ASP.NET Core's IMiddleware.
#[async_trait::async_trait]
pub trait IMiddleware: Send + Sync {
    /// Process an HTTP request (before hook).
    ///
    /// Return `Ok(ControlFlow::Continue(()))` to continue the pipeline,
    /// or `Ok(ControlFlow::Break(()))` to short-circuit (skip remaining
    /// middleware and the final handler; `after` hooks on already-executed
    /// middleware still run in reverse order).
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>>;

    /// Called after the final handler has executed.
    ///
    /// Middleware post-processing, logging, response modification.
    /// Default: no-op.
    async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> {
        Ok(())
    }
}
