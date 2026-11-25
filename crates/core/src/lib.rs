//! Core traits for the rust-webx framework (package `rust-webx-core`, imported
//! as `webx_core`).
//!
//! This crate defines the framework's contracts — `IRequest`, `IRequestHandler`,
//! `IMiddleware`, `IHttpContext`, `IMediator`, `IHostedService`, `Error`,
//! `AppMode` — with **no HTTP engine dependency**. Custom middleware and tests
//! can depend on `webx_core` alone.

// `Error` is now the shared `AppError` carrier (kind/code/detail/fields/
// extensions); boxing it at every signature would add noise without changing
// the wire or the single-carrier contract.
#![allow(clippy::result_large_err)]

pub mod action;
pub mod app;
pub mod auth;
pub mod cache;
pub mod client_info;
pub mod config;
pub mod dispatch_runtime;
pub mod error;
pub mod error_codes;
pub mod form;
pub mod handler;
pub mod hosted;
pub mod http;
pub mod idempotency;
pub mod json;
pub mod keyset;
pub mod localization;
pub mod locking;
pub mod media;
pub mod mediator;
pub mod metrics;
pub mod middleware;
pub mod mode;
pub mod notifications;
pub mod operation;
pub mod options_monitor;
pub mod pagination;
pub mod paths;
pub mod pipeline;
pub mod problem;
pub mod push;
pub mod rate_limit;
pub mod realtime;
pub mod request_context;
pub mod route;
pub mod routing;
pub mod secrets;
pub mod security;
pub mod storage;
pub mod stream;
pub mod time;
pub mod validation;

pub use action::*;
pub use app::*;
pub use auth::*;
pub use cache::cache_ext::DistributedCacheExtensions;
pub use cache::options::DistributedCacheEntryOptions;
pub use cache::trait_def::{CacheError, IDistributedCache};
pub use config::*;
pub use dispatch_runtime::DispatchRuntime;
pub use error::*;
pub use error_codes::{ErrorCodeSpec, ERROR_CODES, FALLBACK_STATUS};
pub use form::*;
pub use handler::*;
pub use hosted::{
    BackgroundService, BackgroundServiceHost, CancellationToken, PeriodicBackgroundService,
    ShutdownToken,
};
pub use http::*;
pub use localization::*;
pub use mediator::*;
pub use metrics::*;
pub use middleware::{
    BoxedNextFuture, IMiddleware, MiddlewareSlot, NextRun, NextStage, TimeoutMiddleware,
};
pub use mode::*;
pub use operation::*;
pub use options_monitor::{IOptionsMonitor, OptionsMonitor};
pub use pagination::*;
pub use paths::*;
pub use pipeline::*;
pub use problem::*;
pub use push::*;
pub use request_context::*;
pub use route::*;
pub use routing::*;
/// Incremental-update contract owned by `rust-core`; re-exported so business
/// crates depend on one crate name (`webx_core::IPatchable`).
pub use rust_core::IPatchable;
pub use secrets::*;
pub use security::*;
pub use stream::*;
