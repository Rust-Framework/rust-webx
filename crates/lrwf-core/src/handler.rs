//! Handler traits: IRequestHandler and IEventHandler.
//!
//! ## IRequestHandler<T, R>
//!
//! Dual-type-parameter handler: `T` is the request type, `R` is the response type.
//! The constraint `T: IRequest<R>` ensures type safety between request and response.
//!
//! ```ignore
//! #[async_trait]
//! impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
//!     async fn handle(&self, req: GetUserRequest) -> Result<UserModel> { ... }
//! }
//! ```

use crate::error::Result;
use crate::mediator::{IEventRequest, IRequest};

/// Handles a single `IRequest<R>`, producing its associated response `R`.
///
/// Register in DI as: `dyn IRequestHandler<GetUserRequest, UserModel>`
///
/// ```ignore
/// struct GetUserHandler { db: Arc<dyn IUserRepository> }
///
/// #[async_trait]
/// impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
///     async fn handle(&self, req: GetUserRequest) -> Result<UserModel> { ... }
/// }
/// ```
#[async_trait::async_trait]
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
{
    async fn handle(&self, req: T) -> Result<R>;
}

/// Handles a single `IEventRequest`, performing side effects.
///
/// ```ignore
/// struct SendWelcomeEmailHandler { email: Arc<dyn IEmailService> }
///
/// #[async_trait]
/// impl IEventHandler<UserCreatedEvent> for SendWelcomeEmailHandler {
///     async fn handle(&self, event: UserCreatedEvent) -> Result<()> { ... }
/// }
/// ```
#[async_trait::async_trait]
pub trait IEventHandler<T: IEventRequest>: Send + Sync {
    async fn handle(&self, event: T) -> Result<()>;
}
