//! IMediator implementation.
//!
//! `Mediator::send` looks up the `HandlerRegistration` (collected via `#[handler]`)
//! by request type name and invokes the same factory + call bridge used by HTTP
//! dispatch. This unifies the HTTP and in-process paths: both obtain an owned
//! handler per call, so `handle(&mut self, ...)` works uniformly.
//!
//! `Mediator::publish` continues to resolve `IEventHandler<T>` from the DI
//! container (events remain `&self` since they typically don't need owned
//! `DbContext` access).

use crate::pipeline::{BoxedNextFn, BoxedPipelineFuture, IPipelineBehavior};
use crate::route::scan::HandlerCache;
use crate::error::{Error, Result};
use crate::handler::IEventHandler;
use super::pipeline::build_chain;
use super::{IEventRequest, IMediator, IRequest};
use rust_dix::{IServiceResolver, ScopeFactory, ServiceProvider};
use std::sync::Arc;

/// Default implementation of IMediator.
///
/// Resolves handlers via the global `HandlerCache` (factory + call bridge)
/// for `send`, and via DI for `publish`.
pub struct Mediator {
    provider: Arc<ServiceProvider>,
}

impl Mediator {
    pub fn new(provider: Arc<ServiceProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl IMediator for Mediator {
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: serde::Serialize + Send + 'static,
    {
        let cache = HandlerCache::get_or_init();
        let full_name = std::any::type_name::<T>();
        // #[handler] registers with the source-code type name (e.g. "HelloRequest"),
        // but type_name returns the full path (e.g. "crate::module::HelloRequest").
        // Try full match first, then fall back to the last segment.
        let entry = cache.get(full_name).or_else(|| {
            let short = full_name.rsplit("::").next().unwrap_or(full_name);
            cache.get(short)
        }).ok_or_else(|| {
            Error::Di(format!(
                "No #[handler] registered for request {} -> {} (looked up as '{}')",
                std::any::type_name::<T>(),
                std::any::type_name::<R>(),
                full_name,
            ))
        })?;

        // Create a per-call scope so Scoped services (e.g. Mutex<DbContext>) resolve
        // to a fresh instance per `send` invocation, matching the HTTP dispatch path
        // which also creates a scope per request (see crates/macros/src/endpoint.rs).
        let scope = self.provider.create_scope();
        let resolver: &dyn IServiceResolver = &scope;
        let handler = (entry.factory)(resolver);

        // Collect registered IPipelineBehavior instances from the scope.
        // Behaviors are typically registered as Singleton; Arc clones keep them alive.
        let behaviors: Vec<Arc<dyn IPipelineBehavior>> = scope.get_all::<dyn IPipelineBehavior>();

        // Terminal closure: captures the owned handler + call bridge.
        // The handler owns its dependencies (Arc<T> clones), so the scope is no longer needed.
        let entry_call = entry.call;
        let terminal: BoxedNextFn = Box::new(
            move |req: Box<dyn std::any::Any + Send>| -> BoxedPipelineFuture {
                Box::pin(async move { (entry_call)(handler, req).await })
            },
        );

        // Build the behavior chain (empty behaviors returns terminal as-is).
        let chain = build_chain(behaviors, terminal);
        let result_box = chain(Box::new(req)).await?;
        let result = *result_box
            .downcast::<R>()
            .expect("Response type mismatch in Mediator::send call bridge");
        Ok(result)
    }

    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()> {
        let handlers: Vec<Arc<dyn IEventHandler<T>>> =
            self.provider.get_all::<dyn IEventHandler<T>>();

        for handler in handlers {
            handler.handle(event.clone()).await?;
        }

        Ok(())
    }
}
