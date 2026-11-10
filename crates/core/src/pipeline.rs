//! Pipeline behavior trait: IPipelineBehavior.

use crate::error::Result;
use std::any::Any;
use std::future::Future;
use std::pin::Pin;

/// Type-erased continuation function for pipeline behaviors.
///
/// Carries a boxed request to the next behavior (or terminal handler).
pub type BoxedNextFn = Box<dyn FnOnce(Box<dyn Any + Send>) -> BoxedPipelineFuture + Send>;

/// Boxed future returned by a pipeline behavior step.
pub type BoxedPipelineFuture = Pin<Box<dyn Future<Output = Result<Box<dyn Any + Send>>> + Send>>;

/// Pipeline behavior that wraps around request handling.
///
/// Multiple behaviors form a chain. Each behavior can:
/// - Inspect or modify the request before passing it on
/// - Inspect or modify the response after it returns
/// - Short-circuit and skip the rest of the chain (by not calling `next`)
///
/// Analogous to MediatR's `IPipelineBehavior<TRequest, TResponse>`.
///
/// # Lifetime and dependencies
///
/// Behaviors are registered as **singletons**: one instance per host, shared by
/// every request. The DI resolver is available only while the behavior is being
/// **constructed** — use [`IServiceCollectionExt::add_pipeline_with`] and take
/// your dependencies in the factory closure:
///
/// ```ignore
/// svc.add_pipeline_with(|resolver| {
///     // `&dyn IServiceResolver` is object-safe: resolve type-erased.
///     let clock: Arc<Clock> = resolver
///         .get_any(std::any::type_name::<Clock>())
///         .and_then(|service| service.downcast::<Arc<Clock>>().ok())
///         .map(|service| Arc::clone(&*service))
///         .expect("register Clock before the pipeline");
///     Arc::new(TimingBehavior { clock }) as Arc<dyn IPipelineBehavior>
/// })
/// ```
///
/// Inside [`IPipelineBehavior::handle`] there is no resolver and no typed
/// request/response: the request and the eventual response are
/// `Box<dyn Any + Send>`. Cross-cutting mechanics (timing, logging, cancellation)
/// belong here; business rules that need the typed request belong in the handler
/// or a use case it calls. [`IServiceCollectionExt::add_pipeline`] remains the
/// zero-dependency convenience registration for `Default` behaviors.
///
/// # Response type contract
///
/// A behavior that short-circuits **must** return the same response type the
/// handler would return (`downcast` to it happens after the chain, before the
/// caller sees the value). Short-circuiting with a different type — for example
/// `Box::new(())` — fails with an explicit type-mismatch error instead of
/// silently succeeding.
///
/// [`IServiceCollectionExt::add_pipeline_with`]: crate::route::ext::IServiceCollectionExt::add_pipeline_with
/// [`IServiceCollectionExt::add_pipeline`]: crate::route::ext::IServiceCollectionExt::add_pipeline
#[async_trait::async_trait]
pub trait IPipelineBehavior: Send + Sync {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>>;
}
