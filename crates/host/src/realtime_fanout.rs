//! Framework event handler for cross-node real-time fan-out (2.3-4b).
//!
//! [`RealtimeFanoutEvent`] travels the configured event transport (in-memory
//! queue, Redis, NATS, ...). Every node's event consumer dispatches it through
//! [`EventDispatcher`](webx_core::mediator::EventDispatcher) to this handler,
//! which writes the payload to the local room connections in
//! [`IConnectionRegistry`].
//!
//! Delivery is best-effort and asynchronous, matching the event transport's
//! at-most-once model; a slow connection drops frames under its backpressure
//! policy instead of failing the event. Local fan-out that needs immediate
//! per-connection results keeps calling `room_connections` directly.

use std::sync::Arc;

use webx_core::error::Result;
use webx_core::handler::IEventHandler;
use webx_core::realtime::{IConnectionRegistry, RealtimeFanoutEvent};

/// Delivers [`RealtimeFanoutEvent`] to this node's connections in a room.
pub struct RealtimeFanoutHandler {
    registry: Arc<dyn IConnectionRegistry>,
}

impl RealtimeFanoutHandler {
    /// Build a handler over the host's connection registry.
    pub fn new(registry: Arc<dyn IConnectionRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl IEventHandler<RealtimeFanoutEvent> for RealtimeFanoutHandler {
    async fn handle(&self, event: RealtimeFanoutEvent) -> Result<()> {
        let payload = event.payload;
        for connection in self.registry.room_connections(&event.room) {
            // Best-effort fan-out: one slow receiver must not fail the event.
            // The connection's backpressure policy accounts for the drop.
            let _ = connection.send_text(payload.clone()).await;
        }
        Ok(())
    }
}
