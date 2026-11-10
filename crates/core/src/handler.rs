//! Handler traits: IRequestHandler and IEventHandler.
//!
//! ## IRequestHandler<T, R>
//!
//! Dual-type-parameter handler: `T` is the request type, `R` is the response type.
//! The constraint `T: IRequest<R>` ensures type safety between request and response.
//!
//! `handle` takes `&mut self` so handlers that own a `DbContext` (resolved via
//! `get_owned` — EFCore-style per-request unit-of-work) can call `ctx.set::<T>()`
//! and `ctx.save_changes()` which require `&mut self`.
//!
//! ```ignore
//! #[async_trait]
//! impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
//!     async fn handle(&mut self, req: GetUserRequest) -> Result<UserModel> { ... }
//! }
//! ```

use crate::error::Result;
use crate::mediator::{IEventRequest, IRequest};

/// Handles a single `IRequest<R>`, producing its associated response `R`.
///
/// Authentication identity is not passed as a method parameter — this trait stays
/// free of auth concerns. Requests that need the caller declare
/// `#[fromclaim] user: IdentityUser`; the binder fills it before `handle` runs.
///
/// Register via `#[handler]` proc macro for compile-time collection,
/// or use `register_handlers!` for manual DI registration.
///
/// `handle(&mut self, ...)` enables the EFCore-style owned-DbContext pattern:
/// handlers declare a bare `ctx: DbContext` field, resolved per-request via
/// `IServiceResolver::get_owned`, and mutate it directly without `Arc<Mutex>`.
///
/// ```ignore
/// #[async_trait]
/// impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
///     async fn handle(&mut self, req: GetUserRequest) -> Result<UserModel> { ... }
/// }
/// ```
#[async_trait::async_trait]
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: Send + 'static,
{
    /// Handle the request.
    async fn handle(&mut self, req: T) -> Result<R>;
}

/// Handles a single `IEventRequest`, performing side effects.
///
/// ```ignore
/// #[async_trait]
/// impl IEventHandler<UserCreatedEvent> for SendWelcomeEmailHandler {
///     async fn handle(&self, event: UserCreatedEvent) -> Result<()> { ... }
/// }
/// ```
#[async_trait::async_trait]
pub trait IEventHandler<T: IEventRequest>: Send + Sync {
    async fn handle(&self, event: T) -> Result<()>;
}

/// Background service that is started when the host starts and
/// stopped when the host performs a graceful shutdown.
///
/// Analogous to ASP.NET Core's IHostedService.
///
/// Use this for:
/// - Data initialization / seeding at application startup
/// - Background polling loops
/// - Queue consumers
/// - Connection pool warmup
///
/// # Example
///
/// ```ignore
/// #[derive(Default)]
/// struct DbInitService;
///
/// #[async_trait]
/// impl IHostedService for DbInitService {
///     async fn start(&self) -> Result<()> {
///         tracing::info!("[DbInitService] Running migrations...");
///         run_migrations().await?;
///         tracing::info!("[DbInitService] Seeding data...");
///         seed_data().await?;
///         Ok(())
///     }
///
///     async fn stop(&self) -> Result<()> {
///         tracing::info!("[DbInitService] Shutting down...");
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait IHostedService: Send + Sync {
    /// Called when the host starts.
    ///
    /// The host waits for all hosted services to finish `start()`
    /// before beginning to accept incoming requests.
    async fn start(&self) -> Result<()>;

    /// Called during a graceful shutdown.
    ///
    /// The host calls `stop()` on all hosted services concurrently
    /// after the HTTP server has stopped accepting new connections.
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
}
