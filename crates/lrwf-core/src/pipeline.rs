//! Pipeline behavior trait: IPipelineBehavior.

use crate::error::Result;
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type-erased continuation function for pipeline behaviors.
///
/// Carries a boxed request and the service resolver for DI lookups.
pub type BoxedNextFn = Box<
    dyn FnOnce(Box<dyn Any + Send>, Arc<dyn IServiceResolver>) -> BoxedPipelineFuture + Send,
>;

/// Boxed future returned by a pipeline behavior step.
pub type BoxedPipelineFuture =
    Pin<Box<dyn Future<Output = Result<Box<dyn Any + Send>>> + Send>>;

/// Service resolver trait for DI lookups within pipeline behaviors.
///
/// This is a minimal trait to avoid a circular dependency between
/// lrwf-core and the DI implementation.
pub trait IServiceResolver: Send + Sync {
    fn get_any(&self, type_name: &str) -> Option<Box<dyn Any + Send>>;
}

/// Pipeline behavior that wraps around request handling.
///
/// Multiple behaviors form a chain. Each behavior can:
/// - Inspect or modify the request before passing it on
/// - Inspect or modify the response after it returns
/// - Short-circuit and skip the rest of the chain
///
/// Analogous to MediatR's IPipelineBehavior<TRequest, TResponse>.
#[async_trait::async_trait]
pub trait IPipelineBehavior: Send + Sync {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
        svc: Arc<dyn IServiceResolver>,
    ) -> Result<Box<dyn Any + Send>>;
}
