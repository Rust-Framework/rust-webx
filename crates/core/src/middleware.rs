//! Middleware trait: IMiddleware.

use crate::error::Result;
use crate::http::IHttpContext;
use std::ops::ControlFlow;

/// Where a user middleware runs relative to the framework's built-in stages.
///
/// The default for [`HostBuilder::use_middleware`] is [`MiddlewareSlot::AfterAuth`],
/// so a middleware that reads the authenticated principal (`ctx.claims()`)
/// actually sees it: authentication runs before the slot. Use
/// [`MiddlewareSlot::BeforeCors`] only when the middleware must observe requests
/// that CORS would otherwise short-circuit (e.g. preflight accounting), and
/// [`MiddlewareSlot::AfterSpa`] for post-static-file hooks.
///
/// [`HostBuilder::use_middleware`]: https://docs.rs/rust-webx-host
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareSlot {
    /// After `RequestId`, before CORS/preflight handling.
    BeforeCors,
    /// After authentication, before the SPA static-file middleware (default).
    AfterAuth,
    /// After the SPA middleware, immediately before the router.
    AfterSpa,
}

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
/// `after` also runs when a middleware's `invoke` or the handler returns an
/// error: the pipeline unwinds the middleware that already entered, so
/// metrics, tracing and cleanup never miss a failed request.
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
