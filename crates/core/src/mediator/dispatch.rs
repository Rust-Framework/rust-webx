//! Shared request dispatch used by `Mediator::send` and HTTP endpoint adapters.
//!
//! Looks up the `HandlerRegistration` (collected via `#[handler]`) by request type,
//! creates a per-request DI scope, runs the `IPipelineBehavior` chain, and invokes
//! the handler through the type-erased call bridge.

use std::any::Any;
use std::sync::Arc;

use rust_dix::ScopeFactory;

use crate::error::Result;
use crate::mediator::IRequest;
use crate::mediator::pipeline::build_chain as build_pipeline_chain;
use crate::pipeline::BoxedNextFn;
use crate::route::scan::HandlerCache;

/// Dispatch a request through the handler registry, pipeline, and call bridge.
pub async fn dispatch<T, R>(provider: &Arc<rust_dix::ServiceProvider>, req: T) -> Result<R>
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
{
    let req_type_id = std::any::TypeId::of::<T>();

    let cache = HandlerCache::get_or_init();
    let entry = cache.get_by_type_id(req_type_id).ok_or_else(|| {
        crate::Error::Di(format!(
            "No #[handler] registered for request type '{}'",
            std::any::type_name::<T>()
        ))
    })?;

    let scope = provider.create_scope();
    let resolver: &dyn rust_dix::IServiceResolver = &scope;
    let handler = (entry.factory)(resolver);

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
    Ok(*result_box
        .downcast::<R>()
        .expect("Response type mismatch in dispatch call bridge"))
}
