//! Shared request dispatch used by `Mediator::send` and HTTP endpoint adapters.
//!
//! Looks up the `HandlerRegistration` (collected via `#[handler]`) by request
//! type, creates a per-request DI scope, runs the pipeline (typed behaviors
//! inside, legacy erased behaviors outside), and invokes the handler.

// The legacy type-erased pipeline is deprecated but exercised on purpose here.
#![allow(deprecated)]

use std::any::Any;
use std::sync::Arc;

use rust_dix::ScopeFactory;

use crate::error::{Error, Result};
use crate::mediator::pipeline::{build_chain as build_erased_chain, build_typed_chain};
use crate::mediator::IRequest;
use crate::pipeline::{
    BoxedNextFn, BoxedPipelineFuture, IErasedPipelineBehavior, IPipelineBehavior,
};
use crate::route::scan::HandlerCache;

/// Dispatch a request through the handler registry, pipeline, and call bridge.
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

    // Typed behaviors are resolved per (request, response) pair; the erased ones
    // are still supported for one release and run as the outer chain.
    let typed_behaviors: Vec<Arc<dyn IPipelineBehavior<T, R>>> =
        scope.get_all::<dyn IPipelineBehavior<T, R>>();
    #[allow(deprecated)]
    let erased_behaviors: Vec<Arc<dyn IErasedPipelineBehavior>> =
        scope.get_all::<dyn IErasedPipelineBehavior>();

    let entry_call = entry.call;

    // Inner chain: handler terminal, wrapped by typed behaviors. The handler is
    // invoked through the compile-time call bridge, so its response arrives
    // erased and is downcast back to `R` here — once, before typed behaviors see
    // it. Typed behaviors themselves are compiler-checked against `R`.
    let typed_chain = build_typed_chain::<T, R>(typed_behaviors, move |typed_req: T| {
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

    // Outer chain: the legacy erased behaviors receive the request boxed and
    // hand it to the typed chain through the terminal below.
    #[allow(deprecated)]
    let erased_terminal: BoxedNextFn =
        Box::new(move |req: Box<dyn Any + Send>| -> BoxedPipelineFuture {
            Box::pin(async move {
                let typed_req = *req.downcast::<T>().map_err(|_| {
                    Error::Internal(format!(
                        "Pipeline request type mismatch: expected `{}`",
                        std::any::type_name::<T>()
                    ))
                })?;
                let out = typed_chain.run(typed_req).await?;
                Ok(Box::new(out) as Box<dyn Any + Send>)
            })
        });

    #[allow(deprecated)]
    let erased_chain = build_erased_chain(erased_behaviors, erased_terminal);
    let result_box = erased_chain(Box::new(req)).await?;
    Ok(*result_box.downcast::<R>().map_err(|_| {
        // Reached when a legacy erased behavior returned a value of a different
        // type than `R`. Typed behaviors cannot cause this: the compiler checks
        // their response type.
        Error::Internal(format!(
            "Response type mismatch: the pipeline returned a different type than the handler \
             expects (expected `{}`). A legacy `IErasedPipelineBehavior` must not short-circuit \
             with a different response type; return the handler's response type or call `next`. \
             Typed `IPipelineBehavior<Req, Rsp>` values are checked at compile time.",
            std::any::type_name::<R>()
        ))
    })?)
}
