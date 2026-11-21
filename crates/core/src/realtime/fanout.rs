//! Cross-node real-time fan-out over the event transport (2.3-4b, converged).
//!
//! Local fan-out stays in [`IConnectionRegistry`](super::IConnectionRegistry):
//! handlers call `room_connections` and write frames directly. **Across
//! processes** the framework uses the existing event transport
//! ([`IEventPublisher`](crate::mediator::IEventPublisher)) instead of a second
//! pub/sub abstraction: publishing [`RealtimeFanoutEvent`] hands the payload to
//! the configured broker (in-memory queue, Redis, NATS, ...), and every node's
//! consumer routes it through [`EventDispatcher`](crate::mediator::EventDispatcher)
//! to the framework-registered fan-out handler, which writes it to that node's
//! room connections.
//!
//! That keeps **one** cross-process transport port, one wire model
//! (`event_name` + JSON payload), and one dispatch/DI mechanism for both domain
//! events and real-time fan-out. The trade-off is the transport's delivery model:
//! asynchronous, **at-most-once**, handler failures logged by the transport.
//! Applications that need stronger guarantees use an outbox plus a durable broker
//! (the event transport already accepts any `IEventPublisher`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use rust_dix::ScopeFactory;

use crate::mediator::{
    dispatch_event_handlers, BoxedEventDispatch, EventRegistration, IEventRequest,
};

/// Stable wire name of [`RealtimeFanoutEvent`].
pub const REALTIME_FANOUT_EVENT_NAME: &str = "webx.realtime.fanout";

/// Broadcast one text payload to every connection in `room` on every node.
///
/// Produced by the framework's cross-node fan-out path; applications normally
/// call `IMediator::publish(RealtimeFanoutEvent::to_room(room, payload))`.
///
/// # Framework-reserved event
///
/// `webx.realtime.fanout` is reserved by the framework: exactly one
/// `IEventHandler<RealtimeFanoutEvent>` (the host's fan-out handler) may be
/// registered. Registering an additional handler for this event would deliver
/// the payload twice. Binary frames need a separate application event with its
/// own handler; this event carries text frames only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeFanoutEvent {
    /// Room whose connections receive the payload.
    pub room: String,
    /// Text frame payload (already encoded for the client).
    pub payload: String,
}

impl RealtimeFanoutEvent {
    /// Stable routing name (override so the wire name never follows the Rust
    /// module path).
    pub const EVENT_NAME: &'static str = REALTIME_FANOUT_EVENT_NAME;

    /// Build a fan-out message for `room`.
    pub fn to_room(room: impl Into<String>, payload: impl Into<String>) -> Self {
        Self {
            room: room.into(),
            payload: payload.into(),
        }
    }
}

impl IEventRequest for RealtimeFanoutEvent {
    fn event_name() -> &'static str {
        Self::EVENT_NAME
    }
}

fn dispatch_realtime_fanout(
    provider: Arc<rust_dix::ServiceProvider>,
    payload: Vec<u8>,
) -> BoxedEventDispatch {
    Box::pin(async move {
        let event: RealtimeFanoutEvent = serde_json::from_slice(&payload).map_err(|error| {
            crate::error::Error::Internal(format!(
                "realtime fan-out payload is not valid JSON: {error}"
            ))
        })?;
        let scope = provider.create_scope();
        dispatch_event_handlers(&scope, event).await
    })
}

// The event system resolves handlers from DI; this registration is what lets a
// broker consumer route the wire message back to the framework fan-out handler.
inventory::submit! {
    EventRegistration {
        event_name: || REALTIME_FANOUT_EVENT_NAME,
        event_type_id: std::any::TypeId::of::<RealtimeFanoutEvent>(),
        dispatch: dispatch_realtime_fanout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mediator::EventMessage;

    #[test]
    fn fanout_event_round_trips_through_the_wire_format() {
        let message = EventMessage::from_event(&RealtimeFanoutEvent::to_room("room-1", "hello"))
            .expect("event encode");
        assert_eq!(message.event_name, "webx.realtime.fanout");
        let decoded: RealtimeFanoutEvent = message.decode().expect("event decode");
        assert_eq!(decoded, RealtimeFanoutEvent::to_room("room-1", "hello"));
    }

    #[test]
    fn the_routing_registration_is_emitted() {
        let registered = inventory::iter::<EventRegistration>
            .into_iter()
            .any(|registration| (registration.event_name)() == RealtimeFanoutEvent::EVENT_NAME);
        assert!(
            registered,
            "the fan-out event must have a routing registration"
        );
    }
}
