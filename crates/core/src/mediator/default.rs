//! IMediator implementation.
//!
//! `Mediator::send` delegates to the shared [`dispatch`](super::dispatch::dispatch)
//! function — the same path used by HTTP endpoint adapters after request construction.
//!
//! `Mediator::publish` resolves `IEventHandler<T>` from the DI container and
//! invokes all registered handlers.

use std::sync::Arc;

use rust_dix::ScopeFactory;

use super::dispatch;
use super::{IEventRequest, IMediator, IRequest};
use crate::error::Result;
use crate::handler::IEventHandler;

/// Default implementation of IMediator.
///
/// Holds a reference to the root `ServiceProvider`. Each `send` call creates
/// a per-request scope so Scoped services (e.g. DbContext) are resolved fresh.
pub struct Mediator {
    provider: Arc<rust_dix::ServiceProvider>,
}

impl Mediator {
    pub fn new(provider: Arc<rust_dix::ServiceProvider>) -> Self {
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
        dispatch::dispatch(&self.provider, req).await
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
