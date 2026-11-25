//! Mediator traits: IMediator, IRequest, IEventRequest.

pub mod default;
pub mod dispatch;
pub mod event;
pub mod pipeline;
pub use default::Mediator;
pub use dispatch::dispatch;
pub use event::{
    dispatch_event_handlers, BoxedEventDispatch, BoxedEventHandlerCall, EventDelivery,
    EventDispatchFn, EventDispatcher, EventHandlerCache, EventHandlerCall, EventHandlerFactory,
    EventHandlerRegistration, EventMessage, EventMetrics, EventMetricsSnapshot, EventRegistration,
    IEventPublisher, InMemoryEventPublisher, InMemoryEventWorker, DEFAULT_EVENT_QUEUE_CAPACITY,
    EVENT_WIRE_VERSION, MAX_EVENT_NAME_BYTES,
};

use crate::error::Result;

/// Marker trait for a request (command or query) carrying a structured response `TResponse`.
///
/// The HTTP adapter serializes `TResponse` as JSON, so it must implement
/// [`serde::Serialize`] — unless it is [`crate::action::Response`], the
/// response-control type used for file downloads and other responses that own
/// their status, headers and body. That is why this trait itself only requires
/// `Send`: the `Serialize` bound is enforced where the value is actually
/// serialized.
///
/// - `TResponse: Serialize` → framework writes JSON and sets status 200
/// - `TResponse = ()`        → framework writes no body and sets status 204
/// - `TResponse = Response`  → the handler builds the whole response
///
/// ```ignore
/// impl IRequest<UserModel> for GetUserRequest {}
/// impl IRequest<()> for DeleteUserRequest {}
/// impl IRequest<Response> for DownloadFileRequest {}
/// ```
pub trait IRequest<TResponse>: Send + 'static
where
    TResponse: Send + 'static,
{
}

/// Marker trait for an event (notification) that does not produce a response.
///
/// Events are **messages**: they are encoded by `publish` and decoded by the
/// event dispatcher, so they must be serde DTOs. That is what lets the same
/// event travel through the default in-memory queue or an external broker
/// (Redis, NATS, ...) without a second event type.
///
/// ```ignore
/// #[derive(Clone, serde::Serialize, serde::Deserialize)]
/// pub struct UserCreatedEvent { pub user_id: String }
///
/// impl IEventRequest for UserCreatedEvent {}
/// ```
///
/// The default [`IEventRequest::event_name`] is the fully-qualified Rust type
/// name. It is the routing key a broker consumer uses, so renaming or moving an
/// event type is a breaking change for messages already in flight; override
/// `event_name` when the wire name must stay stable.
///
/// Serialization is required only where an event crosses a wire boundary
/// ([`EventMessage::from_event`](crate::mediator::event::EventMessage::from_event)
/// / `decode`), not by the trait itself — that keeps existing implementors
/// source-compatible.
pub trait IEventRequest: Clone + Send + 'static {
    /// Stable routing name of this event.
    ///
    /// Override it to decouple the wire name from the Rust module path.
    fn event_name() -> &'static str
    where
        Self: Sized,
    {
        std::any::type_name::<Self>()
    }

    /// Delivery class of this event.
    ///
    /// Defaults to [`EventDelivery::Competing`] (one subscriber per broker
    /// group). Override with [`EventDelivery::Broadcast`] for real-time
    /// fan-out and cross-node control messages: every node must receive its own
    /// copy. The transport routes by class; the wire payload is unchanged.
    fn delivery() -> crate::mediator::event::EventDelivery
    where
        Self: Sized,
    {
        crate::mediator::event::EventDelivery::Competing
    }
}

/// The mediator dispatches requests to their handlers and hands events to the
/// event transport (which delivers them to all registered handlers).
///
/// Analogous to MediatR's IMediator.
///
/// # Object safety
///
/// The trait is dyn-compatible for callers that only need type-erased dispatch:
/// the generic `send`/`publish` methods require `Self: Sized`, while
/// [`IMediator::send_boxed`] takes/returns `Box<dyn Any + Send>` and is the one
/// available on `dyn IMediator`:
///
/// ```ignore
/// let mediator: Arc<dyn IMediator> = Arc::new(Mediator::new(provider));
/// let response = mediator.send_boxed(Box::new(CreateOrderRequest { .. })).await?;
/// let order = *response.downcast::<OrderView>().expect("typed response");
/// ```
#[async_trait::async_trait]
pub trait IMediator: Send + Sync {
    /// Send a request and return its structured response.
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
        Self: Sized;

    /// Send a type-erased request and return a type-erased response.
    ///
    /// Works through `Arc<dyn IMediator>`: the request's `TypeId` is looked up
    /// in the handler registry and the generated boxed dispatch downcasts it,
    /// runs the same pipeline + handler path as [`IMediator::send`], and boxes
    /// the response for the caller to downcast.
    async fn send_boxed(
        &self,
        req: Box<dyn std::any::Any + Send>,
    ) -> Result<Box<dyn std::any::Any + Send>>;

    /// Publish an event for **asynchronous** delivery.
    ///
    /// The event is encoded into an [`EventMessage`] and handed to the
    /// registered [`IEventPublisher`] (the default is the in-memory queue; see
    /// [`IEventPublisher`] and `HostBuilder::use_event_publisher`). The call
    /// returns once the transport accepted the event:
    ///
    /// * it does **not** wait for event handlers, so a slow subscriber cannot
    ///   delay the caller;
    /// * it does **not** return handler errors — those are logged (or retried /
    ///   dead-lettered) by the transport;
    /// * an error from `publish` means the event was *not accepted* (missing
    ///   local registration, full queue, broker unavailable), not that a
    ///   subscriber failed.
    ///
    /// Stays generic (not available on `dyn IMediator`): the event type is
    /// needed to encode it.
    async fn publish<T: IEventRequest + serde::Serialize>(&self, event: T) -> Result<()>
    where
        Self: Sized;
}
