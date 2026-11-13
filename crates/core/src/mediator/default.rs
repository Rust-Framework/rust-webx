//! IMediator implementation.
//!
//! `Mediator::send` delegates to the shared [`dispatch`](super::dispatch::dispatch)
//! function — the same path used by HTTP endpoint adapters after request construction.
//!
//! `Mediator::publish` resolves `IEventHandler<T>` from the DI container and
//! invokes all registered handlers.

use std::any::Any;
use std::sync::Arc;

use rust_dix::ScopeFactory;

use super::dispatch;
use super::{IEventRequest, IMediator, IRequest};
use crate::error::Result;
use crate::handler::IEventHandler;
use crate::route::scan::HandlerCache;

/// Default implementation of IMediator.
///
/// Holds the host's root `ServiceProvider` and its compiled handler registry.
/// Each `send` call creates a per-request scope, so Scoped services (e.g.
/// DbContext) are resolved fresh and released with the scope by rust-dix.
pub struct Mediator {
    provider: Arc<rust_dix::ServiceProvider>,
    handler_cache: Arc<HandlerCache>,
}

impl Mediator {
    /// Create a mediator over a host's provider and compiled handler registry.
    ///
    /// `Host::build` constructs these handles once;
    /// [`IServiceCollectionExt::add_mediator`](crate::route::ext::IServiceCollectionExt::add_mediator)
    /// registers a transient mediator that resolves them from DI, so handler
    /// code can inject `Arc<dyn IMediator>` / `Mediator` and use the same path
    /// as in-process calls.
    pub fn new(provider: Arc<rust_dix::ServiceProvider>, handler_cache: Arc<HandlerCache>) -> Self {
        Self {
            provider,
            handler_cache,
        }
    }
}

#[async_trait::async_trait]
impl IMediator for Mediator {
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
    {
        dispatch::dispatch(&self.provider, &self.handler_cache, req).await
    }

    async fn send_boxed(&self, req: Box<dyn Any + Send>) -> Result<Box<dyn Any + Send>> {
        let type_id = (*req).type_id();
        let dispatch_fn = self.handler_cache.boxed_dispatch(type_id).ok_or_else(|| {
            crate::Error::Di(format!(
                "No #[handler] with a boxed dispatch bridge is registered for request type {:?}",
                type_id
            ))
        })?;
        dispatch_fn(
            Arc::clone(&self.provider),
            Arc::clone(&self.handler_cache),
            req,
        )
        .await
    }

    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()> {
        let scope = self.provider.create_scope();
        let handlers: Vec<Arc<dyn IEventHandler<T>>> = scope.get_all::<dyn IEventHandler<T>>();

        let mut first_err: Option<crate::error::Error> = None;
        for handler in handlers {
            if let Err(e) = handler.handle(event.clone()).await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
