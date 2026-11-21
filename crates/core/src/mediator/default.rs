//! IMediator implementation.
//!
//! `Mediator::send` delegates to the shared [`dispatch`](super::dispatch::dispatch)
//! function — the same path used by HTTP endpoint adapters after request construction.
//!
//! `Mediator::publish` encodes the event and hands it to the registered
//! [`IEventPublisher`]. Handler execution is owned by the transport (the default
//! in-memory queue), so publishing never waits for subscribers and never returns
//! their errors.

use std::any::Any;
use std::sync::Arc;

use super::dispatch;
use super::event::{EventMessage, EventMetrics, IEventPublisher};
use super::{IEventRequest, IMediator, IRequest};
use crate::error::{Error, Result};
use crate::route::scan::HandlerCache;

/// Default implementation of IMediator.
///
/// Holds the host's root `ServiceProvider` and its compiled handler registry.
/// Each `send` call creates a per-request scope, so Scoped services (e.g.
/// DbContext) are resolved fresh and released with the scope by rust-dix.
pub struct Mediator {
    provider: Arc<rust_dix::ServiceProvider>,
    handler_cache: Arc<HandlerCache>,
    event_metrics: Arc<EventMetrics>,
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
        let event_metrics = provider
            .get::<EventMetrics>()
            .unwrap_or_else(|_| Arc::new(EventMetrics::new()));
        Self {
            provider,
            handler_cache,
            event_metrics,
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

    async fn publish<T: IEventRequest + serde::Serialize>(&self, event: T) -> Result<()> {
        // Resolve before encoding: an unconfigured publisher should fail fast
        // instead of doing serialization work it will throw away.
        let publisher: Arc<dyn IEventPublisher> = match self.provider.get::<dyn IEventPublisher>() {
            Ok(publisher) => publisher,
            Err(_) => {
                self.event_metrics.record_publish_failed();
                return Err(Error::Di(
                    "No IEventPublisher is registered. Use `add_mediator()` (default \
                     in-memory queue) or `HostBuilder::use_event_publisher(...)`."
                        .to_string(),
                ));
            }
        };

        let message = match EventMessage::from_event(&event) {
            Ok(message) => message,
            Err(error) => {
                self.event_metrics.record_publish_failed();
                return Err(error);
            }
        };
        match publisher.publish(message).await {
            Ok(()) => {
                self.event_metrics.record_published();
                Ok(())
            }
            Err(error) => {
                self.event_metrics.record_publish_failed();
                Err(error)
            }
        }
    }
}
