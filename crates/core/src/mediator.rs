//! Mediator traits: IMediator, IRequest, IEventRequest.

pub mod default;
pub mod pipeline;
pub use default::Mediator;

use crate::error::Result;

/// Marker trait for a request (command or query) carrying a structured response `TResponse`.
///
/// - `TResponse: Serialize` → framework writes JSON and sets status 200
/// - `TResponse = ()`        → framework writes no body and sets status 204
///
/// ```ignore
/// impl IRequest<UserModel> for GetUserRequest {}
/// impl IRequest<()> for DeleteUserRequest {}
/// ```
pub trait IRequest<TResponse>: Send + 'static
where
    TResponse: serde::Serialize + Send + 'static,
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
#[async_trait::async_trait]
pub trait IMediator: Send + Sync {
    /// Send a request and return its structured response.
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: serde::Serialize + Send + 'static;

    /// Publish an event to all registered handlers.
    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()>;
}
