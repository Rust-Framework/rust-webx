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
/// Behaviors resolve dependencies via constructor injection (registered as
/// Singleton in the DI container), following the MediatR pattern.
///
/// Analogous to MediatR's IPipelineBehavior<TRequest, TResponse>.
#[async_trait::async_trait]
pub trait IPipelineBehavior: Send + Sync {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>>;
}
