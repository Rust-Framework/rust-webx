//! IMediator implementation.
//!
//! The `Mediator` resolves `IRequestHandler<T, R>` from the DI container
//! and dispatches requests/events.

use lrdi::ServiceProvider;
use crate::error::{Error, Result};
use crate::handler::{IEventHandler, IRequestHandler};
use crate::mediator::{IEventRequest, IMediator, IRequest};
use std::sync::Arc;

/// Default implementation of IMediator.
///
/// Resolves handlers from the DI container and dispatches.
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
        let handler = self
            .provider
            .get_service::<dyn IRequestHandler<T, R>>()
            .ok_or_else(|| {
                Error::Di(format!(
                    "No handler registered for request {} → {}",
                    std::any::type_name::<T>(),
                    std::any::type_name::<R>(),
                ))
            })?;

        handler.handle(req).await
    }

    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()> {
        let handlers: Vec<Arc<dyn IEventHandler<T>>> = self
            .provider
            .get_all::<dyn IEventHandler<T>>();

        for handler in handlers {
            handler.handle(event.clone()).await?;
        }

        Ok(())
    }
}
