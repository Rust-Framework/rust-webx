//! Mediator traits: IMediator, IRequest, IEventRequest.

pub mod default;
pub mod dispatch;
pub mod pipeline;
pub use default::Mediator;
pub use dispatch::dispatch;
pub use pipeline::build_chain as build_pipeline_chain;

use crate::error::Result;

/// Marker trait for a request (command or query) carrying a structured response `TResponse`.
///
/// The HTTP adapter serializes `TResponse` as JSON, so it must implement
/// [`serde::Serialize`] — unless it is [`crate::route::scan::ResponseData`],
/// the raw-response envelope used for file downloads and other responses that
/// control their own status, headers and body. That is why this trait itself
/// only requires `Send`: the `Serialize` bound is enforced where the value is
/// actually serialized.
///
/// - `TResponse: Serialize` → framework writes JSON and sets status 200
/// - `TResponse = ()`        → framework writes no body and sets status 204
/// - `TResponse = ResponseData` → the handler builds the whole response
///
/// ```ignore
/// impl IRequest<UserModel> for GetUserRequest {}
/// impl IRequest<()> for DeleteUserRequest {}
/// impl IRequest<ResponseData> for DownloadFileRequest {}
/// ```
pub trait IRequest<TResponse>: Send + 'static
where
    TResponse: Send + 'static,
{
}

/// Marker trait for an event (notification) that does not produce a response.
///
/// Use `IEventRequest` for fire-and-forget notifications.
///
/// ```ignore
/// impl IEventRequest for UserCreatedEvent {}
/// ```
pub trait IEventRequest: Clone + Send + 'static {}

/// The mediator dispatches requests to their handlers and publishes events
/// to all registered handlers.
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

    /// Publish an event to all registered handlers.
    ///
    /// Stays generic (not available on `dyn IMediator`): event handlers are
    /// resolved from DI by their concrete `IEventHandler<T>` type.
    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()>
    where
        Self: Sized;
}
