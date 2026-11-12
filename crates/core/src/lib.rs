//! Core traits for the rust-webx framework (package `rust-webx-core`, imported
//! as `webx_core`).
//!
//! This crate defines the framework's contracts — `IRequest`, `IRequestHandler`,
//! `IMiddleware`, `IHttpContext`, `IMediator`, `IHostedService`, `Error`,
//! `AppMode` — with **no HTTP engine dependency**. Custom middleware and tests
//! can depend on `webx_core` alone.

pub mod action;
pub mod app;
pub mod auth;
pub mod cache;
pub mod config;
pub mod dispatch_runtime;
pub mod error;
pub mod form;
pub mod handler;
pub mod http;
pub mod localization;
pub mod mediator;
pub mod middleware;
pub mod mode;
pub mod operation;
pub mod pagination;
pub mod paths;
pub mod pipeline;
pub mod problem;
pub mod request_context;
pub mod route;
pub mod routing;

pub use action::*;
pub use app::*;
pub use auth::*;
pub use cache::cache_ext::DistributedCacheExtensions;
pub use cache::options::DistributedCacheEntryOptions;
pub use cache::trait_def::{CacheError, IDistributedCache};
pub use config::*;
pub use dispatch_runtime::DispatchRuntime;
pub use error::*;
pub use form::*;
pub use handler::*;
pub use http::*;
pub use localization::*;
pub use mediator::*;
pub use middleware::*;
pub use mode::*;
pub use operation::*;
pub use pagination::*;
pub use paths::*;
pub use pipeline::*;
pub use problem::*;
pub use request_context::*;
pub use route::*;
pub use routing::*;
