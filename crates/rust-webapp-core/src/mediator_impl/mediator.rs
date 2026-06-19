//! IMediator implementation.
//!
//! The `Mediator` dispatches requests via the HandlerCache (compile-time registry)
//! instead of runtime DI lookups. IEventHandler dispatch is kept via rust_dicore DI
//! for backward compatibility (IMiddleware still uses dyn dispatch).

use crate::di::scan::HandlerCache;
use crate::error::{Error, Result};
use crate::handler::IEventHandler;
use crate::mediator::{IEventRequest, IMediator, IRequest};
use rust_dicore::ServiceProvider;
use std::sync::Arc;

/// Default implementation of IMediator.
///
/// Uses the HandlerCache for O(1) request dispatch.
pub struct Mediator {
    cache: Arc<HandlerCache>,
    /// Kept for IEventHandler resolution (still dyn-based via rust_dicore DI)
    provider: Arc<ServiceProvider>,
}

impl Mediator {
    pub fn new(cache: Arc<HandlerCache>, provider: Arc<ServiceProvider>) -> Self {
        Self { cache, provider }
    }
}

#[async_trait::async_trait]
impl IMediator for Mediator {
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
    {
        let req_type_name = std::any::type_name::<T>();
        let entry = self.cache.get(req_type_name).ok_or_else(|| {
            Error::Di(format!(
                "No handler registered for request {} â†’ {}",
                req_type_name,
                std::any::type_name::<R>(),
            ))
        })?;

        // Box the request for the type-erased call bridge
        let request_boxed: Box<dyn std::any::Any + Send> = Box::new(req);

        // Call through the type-erased bridge
        let response = (entry.call)(&entry.handler, request_boxed, None).await?;

        // Deserialize the response
        let result: R = serde_json::from_slice(&response.body).map_err(Error::Serialization)?;
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
