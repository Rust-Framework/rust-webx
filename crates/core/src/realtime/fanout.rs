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
    dispatch_event_handlers, event::EventDelivery, BoxedEventDispatch, EventHandlerRegistration,
    EventRegistration, IEventRequest,
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

    /// Fan-out is broadcast-class: every node must deliver its own copy to the
    /// room's local connections. Competing delivery would reach exactly one
    /// node.
    fn delivery() -> EventDelivery {
        EventDelivery::Broadcast
    }
}

fn dispatch_realtime_fanout(
    provider: Arc<rust_dix::ServiceProvider>,
    payload: Vec<u8>,
) -> BoxedEventDispatch {
    Box::pin(async move {
        let event: RealtimeFanoutEvent = serde_json::from_slice(&payload).map_err(|error| {
            crate::error::Error::internal(format!(
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

/// Stable wire name of [`RealtimeUserFanoutEvent`].
pub const REALTIME_USER_FANOUT_EVENT_NAME: &str = "webx.realtime.user-fanout";

/// Broadcast one text payload to every connection of one user, on every node.
///
/// A room is not always the right address: 1:1 signaling, direct messages and
/// "kick this device" control messages target a **user**, and the user may have
/// connections on several nodes. Publishing through the broadcast-class event
/// transport reaches every node; each node writes the payload to its local
/// `user_connections(user_id)`, which covers all of that user's devices.
///
/// The event is framework-reserved exactly like [`RealtimeFanoutEvent`]: one
/// registered handler (the host's) consumes it, and it carries text frames only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeUserFanoutEvent {
    /// Authenticated user id whose local connections receive the payload.
    pub user_id: String,
    /// Text frame payload (already encoded for the client).
    pub payload: String,
}

impl RealtimeUserFanoutEvent {
    /// Stable routing name.
    pub const EVENT_NAME: &'static str = REALTIME_USER_FANOUT_EVENT_NAME;

    /// Build a user-targeted fan-out message.
    pub fn to_user(user_id: impl Into<String>, payload: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            payload: payload.into(),
        }
    }
}

impl IEventRequest for RealtimeUserFanoutEvent {
    fn event_name() -> &'static str {
        Self::EVENT_NAME
    }

    fn delivery() -> EventDelivery {
        EventDelivery::Broadcast
    }
}

fn dispatch_realtime_user_fanout(
    provider: Arc<rust_dix::ServiceProvider>,
    payload: Vec<u8>,
) -> BoxedEventDispatch {
    Box::pin(async move {
        let event: RealtimeUserFanoutEvent = serde_json::from_slice(&payload).map_err(|error| {
            crate::error::Error::internal(format!(
                "realtime user fan-out payload is not valid JSON: {error}"
            ))
        })?;
        let scope = provider.create_scope();
        dispatch_event_handlers(&scope, event).await
    })
}

// Same one-handler rule as the room fan-out event: the registration lets a
// broker consumer route the wire message back to the framework handler.
inventory::submit! {
    EventRegistration {
        event_name: || REALTIME_USER_FANOUT_EVENT_NAME,
        event_type_id: std::any::TypeId::of::<RealtimeUserFanoutEvent>(),
        dispatch: dispatch_realtime_user_fanout,
    }
}

/// Fail closed when an application `#[handler]` claims one of the
/// framework-reserved realtime fan-out events.
///
/// The host registers `RealtimeFanoutHandler` / `RealtimeUserFanoutHandler` in DI
/// as the single subscriber of these events. Adding an application
/// `#[handler] impl IEventHandler<RealtimeFanoutEvent>` emits an inventory
/// registration for the same event name; [`dispatch_event_handlers`] prefers
/// inventory handlers over DI, so the framework bridge would be silently
/// replaced and cross-node fan-out would stop. Host construction calls this
/// before building the provider and reports a typed, actionable error.
///
/// Applications that need their own cross-node event use a distinct
/// `IEventRequest` type; they must not subscribe to the reserved wire names.
pub fn validate_reserved_event_handlers() -> crate::error::Result<()> {
    for registration in inventory::iter::<EventHandlerRegistration> {
        let event_name = (registration.event_name)();
        if event_name == REALTIME_FANOUT_EVENT_NAME || event_name == REALTIME_USER_FANOUT_EVENT_NAME
        {
            return Err(crate::error::Error::di(format!(
                "event '{event_name}' is framework-reserved for realtime fan-out: remove the \
                 #[handler] from its IEventHandler impl (handler '{}'); the host's fan-out bridge \
                 must stay the only subscriber. Use a distinct event type for application payloads",
                registration.handler_name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mediator::EventMessage;

    #[test]
    fn no_application_handler_claims_the_reserved_fanout_events() {
        validate_reserved_event_handlers()
            .expect("the framework core has no reserved-event handler");
    }

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

    #[test]
    fn the_user_fanout_registration_is_emitted_and_round_trips() {
        let registered = inventory::iter::<EventRegistration>
            .into_iter()
            .any(|registration| (registration.event_name)() == RealtimeUserFanoutEvent::EVENT_NAME);
        assert!(
            registered,
            "the user fan-out event must have a routing registration"
        );
        let message =
            EventMessage::from_event(&RealtimeUserFanoutEvent::to_user("user-1", "hello"))
                .expect("event encode");
        let decoded: RealtimeUserFanoutEvent = message.decode().expect("event decode");
        assert_eq!(decoded, RealtimeUserFanoutEvent::to_user("user-1", "hello"));
    }

    #[test]
    fn both_fanout_events_are_broadcast_class() {
        use crate::mediator::event::EventDelivery;
        assert_eq!(
            <RealtimeFanoutEvent as IEventRequest>::delivery(),
            EventDelivery::Broadcast
        );
        assert_eq!(
            <RealtimeUserFanoutEvent as IEventRequest>::delivery(),
            EventDelivery::Broadcast
        );
    }
}
