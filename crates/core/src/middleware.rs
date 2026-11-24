//! Middleware trait: IMiddleware.

use crate::error::Result;
use crate::http::IHttpContext;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;

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

/// Boxed future produced by a [`NextStage`] continuation.
pub type BoxedNextFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// Boxed continuation body stored by [`NextStage`].
pub type NextRun =
    Box<dyn for<'ctx> FnOnce(&'ctx mut dyn IHttpContext) -> BoxedNextFuture<'ctx> + Send>;

/// Continuation handle for the remaining middleware chain (2.4-4).
///
/// A continuation-style middleware receives `NextStage`, decides whether to continue
/// (`next.run(ctx).await`), and can wrap the call for timeout/retry/circuit
/// breaking. Not calling `next.run` short-circuits the remaining stages and the
/// terminal handler. The handle is consumed by `run`, so a continuation can only
/// be advanced once (same rule as ASP.NET Core's `RequestDelegate`).
pub struct NextStage {
    run: NextRun,
}

impl NextStage {
    /// Build a continuation from the pipeline executor.
    pub fn new<F>(run: F) -> Self
    where
        F: for<'ctx> FnOnce(&'ctx mut dyn IHttpContext) -> BoxedNextFuture<'ctx> + Send + 'static,
    {
        Self { run: Box::new(run) }
    }

    /// Advance the chain.
    pub async fn run(self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let run = self.run;
        run(ctx).await
    }
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
/// # Continuation style (2.4-4)
///
/// Override [`Self::invoke_next`] (and return `true` from
/// [`Self::continuation`]) to receive a [`NextStage`] continuation instead of the
/// legacy `invoke`/`after` pair:
///
/// ```ignore
/// #[async_trait::async_trait]
/// impl IMiddleware for TimeoutMiddleware {
///     async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
///         unreachable!("continuation-style middleware implements invoke_next")
///     }
///     fn continuation(&self) -> bool { true }
///     async fn invoke_next(&self, ctx: &mut dyn IHttpContext, next: NextStage) -> Result<()> {
///         tokio::time::timeout(self.timeout, next.run(ctx))
///             .await
///             .map_err(|_| Error::status(504, "timeout"))?
///     }
/// }
/// ```
///
/// The default [`Self::invoke_next`] bridges to `invoke`/`after`, so every
/// existing middleware keeps working unchanged. The engine only pays for the
/// continuation machinery when at least one stage declares `continuation()`.
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

    /// Whether this stage has a meaningful `after` hook (2.4-9).
    ///
    /// The default is `true` so user middleware that overrides `after` keeps
    /// working without changes. Built-in header-only stages return `false`,
    /// which lets the engine skip the boxed `after` future entirely.
    fn has_after(&self) -> bool {
        true
    }

    /// Synchronous fast path for middleware that performs no I/O (2.4-9).
    ///
    /// Built-in stages (request id, CORS, security headers) are pure header /
    /// context mutation. Returning `Some` here lets the engine call them
    /// without the `async_trait` boxed future, removing one allocation per
    /// stage from the default pipeline. Default `None` keeps the async path.
    fn fast_invoke(&self, _ctx: &mut dyn IHttpContext) -> Option<Result<ControlFlow<()>>> {
        None
    }

    /// Whether this stage uses the continuation style (2.4-4).
    ///
    /// Return `true` when [`Self::invoke_next`] is overridden; the engine then
    /// selects the continuation executor for the whole pipeline. Default
    /// `false` keeps the zero-allocation legacy loop.
    fn continuation(&self) -> bool {
        false
    }

    /// Continuation-style execution (2.4-4).
    ///
    /// Default: legacy `invoke` semantics — `Break` short-circuits, `Continue`
    /// advances `next` and then runs `after` (also when the rest of the chain
    /// failed, so post-processing still observes the error response).
    async fn invoke_next(&self, ctx: &mut dyn IHttpContext, next: NextStage) -> Result<()> {
        match self.invoke(ctx).await {
            Ok(ControlFlow::Break(())) => Ok(()),
            Ok(ControlFlow::Continue(())) => {
                let result = next.run(ctx).await;
                match self.after(ctx).await {
                    Ok(()) => result,
                    Err(after_error) => {
                        if result.is_ok() {
                            Err(after_error)
                        } else {
                            tracing::error!(error = %after_error, "middleware after hook failed");
                            result
                        }
                    }
                }
            }
            Err(error) => Err(error),
        }
    }
}

/// Continuation-style timeout middleware (2.4-4).
///
/// Wraps the rest of the pipeline in `tokio::time::timeout`; a slow handler is
/// cancelled and answered with `504 Gateway Timeout`. It demonstrates the
/// continuation composition the `Next` API enables (retry/circuit-breaker
/// middlewares follow the same shape).
pub struct TimeoutMiddleware {
    timeout: std::time::Duration,
}

impl TimeoutMiddleware {
    /// Time out the remaining chain after `timeout`.
    pub fn new(timeout: std::time::Duration) -> Self {
        Self { timeout }
    }
}

#[async_trait::async_trait]
impl IMiddleware for TimeoutMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // Continuation-style stages are driven through `invoke_next`.
        Ok(ControlFlow::Continue(()))
    }

    fn continuation(&self) -> bool {
        true
    }

    async fn invoke_next(&self, ctx: &mut dyn IHttpContext, next: NextStage) -> Result<()> {
        match tokio::time::timeout(self.timeout, next.run(ctx)).await {
            Ok(result) => result,
            Err(_) => {
                ctx.response_mut().set_status(504);
                Ok(())
            }
        }
    }
}
