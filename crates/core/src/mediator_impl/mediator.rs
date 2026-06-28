//! IMediator implementation.
//!
//! The `Mediator` resolves `IRequestHandler<T, R>` and `IEventHandler<T>`
//! from the rust_dicore DI container. Handlers are registered via
//! `#[rust_dicore::inject]` on their impl blocks, unifying service
//! registration under a single attribute macro.

use crate::error::{Error, Result};
use crate::handler::{IEventHandler, IRequestHandler};
use crate::mediator::{IEventRequest, IMediator, IRequest};
use rust_dicore::ServiceProvider;
use std::sync::Arc;

/// Default implementation of IMediator.
///
/// Resolves handlers from the DI container (`ServiceProvider`).
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
        let handler: Arc<dyn IRequestHandler<T, R>> = self
            .provider
            .get_optional::<dyn IRequestHandler<T, R>>()
            .ok_or_else(|| {
                Error::Di(format!(
                    "No handler registered for request {} -> {}",
                    std::any::type_name::<T>(),
                    std::any::type_name::<R>(),
                ))
            })?;
        handler.handle(req).await
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
