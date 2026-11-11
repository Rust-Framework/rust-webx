//! Shared request dispatch used by `Mediator::send` and HTTP endpoint adapters.
//!
//! Looks up the `HandlerRegistration` (collected via `#[handler]`) by request
//! type, creates a per-request DI scope, runs the typed `IPipelineBehavior`
//! chain, and invokes the handler through the type-erased call bridge.

use std::any::Any;
use std::sync::Arc;

use rust_dix::ScopeFactory;

use crate::error::{Error, Result};
use crate::mediator::pipeline::build_typed_chain;
use crate::mediator::IRequest;
use crate::pipeline::IPipelineBehavior;
use crate::route::scan::HandlerCache;

/// Dispatch a request through the handler registry, typed pipeline and call bridge.
pub async fn dispatch<T, R>(
    provider: &Arc<rust_dix::ServiceProvider>,
    handler_cache: &HandlerCache,
    req: T,
) -> Result<R>
where
    T: IRequest<R> + Send + 'static,
    R: Send + 'static,
{
    let req_type_id = std::any::TypeId::of::<T>();

    let entry = handler_cache.get_by_type_id(req_type_id).ok_or_else(|| {
        Error::Di(format!(
            "No #[handler] registered for request type '{}'",
            std::any::type_name::<T>()
        ))
    })?;

    let scope = provider.create_scope();
    let resolver: &dyn rust_dix::IServiceResolver = &scope;
    let handler = (entry.factory)(resolver)?;

    // Behaviors are resolved per (request, response) pair inside the request
    // scope; the chain is fully typed, so the handler's response needs no
    // downcast after the terminal.
    let behaviors: Vec<Arc<dyn IPipelineBehavior<T, R>>> =
        scope.get_all::<dyn IPipelineBehavior<T, R>>();

    let entry_call = entry.call;
    let chain = build_typed_chain::<T, R>(behaviors, move |typed_req: T| {
        Box::pin(async move {
            let out = (entry_call)(handler, Box::new(typed_req) as Box<dyn Any + Send>).await?;
            out.downcast::<R>().map(|value| *value).map_err(|_| {
                Error::Internal(format!(
                    "Handler for `{}` returned a different response type than `{}`",
                    std::any::type_name::<T>(),
                    std::any::type_name::<R>()
                ))
            })
        })
    });

    chain.run(req).await
}
