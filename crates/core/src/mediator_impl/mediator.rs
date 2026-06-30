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

use crate::di::scan::HandlerCache;
use crate::error::{Error, Result};
use crate::handler::IEventHandler;
use crate::mediator::{IEventRequest, IMediator, IRequest};
use rust_dicore::{IServiceResolver, ServiceProvider};
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
        R: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
    {
        let cache = HandlerCache::get_or_init();
        let type_name = std::any::type_name::<T>();
        let entry = cache.get(type_name).ok_or_else(|| {
            Error::Di(format!(
                "No #[handler] registered for request {} -> {} (looked up as '{}')",
                std::any::type_name::<T>(),
                std::any::type_name::<R>(),
                type_name,
            ))
        })?;

        // Use the root provider as resolver. Scoped services resolved from root
        // degrade to transient (fresh instance per call) per rust-dicore 0.5
        // semantics — sufficient for in-process Mediator dispatch.
        let resolver: &dyn IServiceResolver = self.provider.as_ref();
        let handler = (entry.factory)(resolver);
        let result_box = (entry.call)(handler, Box::new(req)).await?;
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
