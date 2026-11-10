//! Shared request dispatch used by `Mediator::send` and HTTP endpoint adapters.
//!
//! Looks up the `HandlerRegistration` (collected via `#[handler]`) by request type,
//! creates a per-request DI scope, runs the `IPipelineBehavior` chain, and invokes
//! the handler through the type-erased call bridge.

use std::any::Any;
use std::sync::Arc;

use rust_dix::ScopeFactory;

use crate::dispatch_runtime::dispatch_handler_cache;
use crate::error::Result;
use crate::mediator::pipeline::build_chain as build_pipeline_chain;
use crate::mediator::IRequest;
use crate::pipeline::BoxedNextFn;

/// Dispatch a request through the handler registry, pipeline, and call bridge.
pub async fn dispatch<T, R>(provider: &Arc<rust_dix::ServiceProvider>, req: T) -> Result<R>
where
    T: IRequest<R> + Send + 'static,
    R: Send + 'static,
{
    let req_type_id = std::any::TypeId::of::<T>();

    let cache = dispatch_handler_cache();
    let entry = cache.get_by_type_id(req_type_id).ok_or_else(|| {
        crate::Error::Di(format!(
            "No #[handler] registered for request type '{}'",
            std::any::type_name::<T>()
        ))
    })?;

    let scope = provider.create_scope();
    let resolver: &dyn rust_dix::IServiceResolver = &scope;
    let handler = (entry.factory)(resolver)?;

    let behaviors: Vec<Arc<dyn crate::pipeline::IPipelineBehavior>> =
        scope.get_all::<dyn crate::pipeline::IPipelineBehavior>();

    let entry_call = entry.call;
    let terminal: BoxedNextFn = Box::new(
        move |req: Box<dyn Any + Send>| -> crate::pipeline::BoxedPipelineFuture {
            Box::pin(async move { (entry_call)(handler, req).await })
        },
    );

    let chain = build_pipeline_chain(behaviors, terminal);
    let result_box = chain(Box::new(req)).await?;
    Ok(*result_box.downcast::<R>().map_err(|_| {
        // Reached when something in the chain returned a value of a different
        // type than `R`. The handler's own response type is `R`; the usual
        // cause is a pipeline behavior that short-circuited with a placeholder
        // (e.g. `Box::new(())`) instead of the handler's response type.
        crate::Error::Internal(format!(
            "Response type mismatch: the pipeline returned a different type than the handler \
             expects (expected `{}`). A pipeline behavior must not short-circuit with a \
             different response type (e.g. `Box::new(())`); return the handler's response type \
             or call `next`.",
            std::any::type_name::<R>()
        ))
    })?)
}
