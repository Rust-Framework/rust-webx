//! Typed pipeline behaviors: `IPipelineBehavior<Req, Rsp>`.

use crate::error::{ActionResult, Result};
use std::future::Future;
use std::pin::Pin;

/// Boxed future returned by one typed pipeline step.
type NextFuture<Rsp> = Pin<Box<dyn Future<Output = Result<Rsp>> + Send>>;

/// Callable continuation stored by [`Next`].
type NextCall<Req, Rsp> = Box<dyn FnOnce(Req) -> NextFuture<Rsp> + Send>;

/// The continuation a behavior calls to run the rest of the chain.
///
/// The framework builds this value; behaviors only consume it:
///
/// ```ignore
/// let response = next.run(req).await?;   // run the remaining chain + handler
/// ```
pub struct Next<Req, Rsp>
where
    Req: Send + 'static,
    Rsp: Send + 'static,
{
    call: NextCall<Req, Rsp>,
}

impl<Req, Rsp> Next<Req, Rsp>
where
    Req: Send + 'static,
    Rsp: Send + 'static,
{
    /// Wrap a typed continuation.
    pub fn new<F, Fut>(call: F) -> Self
    where
        F: FnOnce(Req) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Rsp>> + Send + 'static,
    {
        Self {
            call: Box::new(move |req| Box::pin(call(req))),
        }
    }

    /// Run the rest of the chain (and the handler) with `req`.
    pub async fn run(self, req: Req) -> ActionResult<Rsp> {
        (self.call)(req).await
    }
}

/// Typed pipeline behavior that wraps around request handling.
///
/// Multiple behaviors form a chain: the first registered behavior is the
/// outermost (runs first on the request, last on the response). A behavior can
/// inspect/modify the request and response, or **short-circuit** by returning a
/// response without calling [`Next::run`] — the value is the handler's response
/// type, checked by the compiler:
///
/// ```ignore
/// #[derive(Default)]
/// struct RequireNotes;
///
/// #[async_trait::async_trait]
/// impl IPipelineBehavior<CreateOrderRequest, OrderView> for RequireNotes {
///     async fn handle(
///         &self,
///         req: CreateOrderRequest,
///         next: Next<CreateOrderRequest, OrderView>,
///     ) -> ActionResult<OrderView> {
///         if req.note.is_empty() {
///             return Err(Error::Validation("note is required".into()));
///         }
///         next.run(req).await
///     }
/// }
///
/// svc.add_typed_pipeline::<RequireNotes, CreateOrderRequest, OrderView>();
/// ```
///
/// Behaviors are **singletons** (one instance per host). The DI resolver is only
/// available while a behavior is being **constructed**:
/// [`IServiceCollectionExt::add_typed_pipeline_with`] hands the factory the
/// resolver; inside `handle` only the typed request/continuation exist.
///
/// [`IServiceCollectionExt::add_typed_pipeline_with`]:
///     crate::route::ext::IServiceCollectionExt::add_typed_pipeline_with
#[async_trait::async_trait]
pub trait IPipelineBehavior<Req: Send + 'static, Rsp: Send + 'static>: Send + Sync {
    async fn handle(&self, req: Req, next: Next<Req, Rsp>) -> ActionResult<Rsp>;
}
