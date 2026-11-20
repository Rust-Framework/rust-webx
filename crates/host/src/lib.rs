//! HTTP server, middleware pipeline, and routing (package `rust-webx-host`,
//! imported as `webx_host`).
//!
//! Implements the [`webx_core`] contracts on the 2.0 worker engine: the `Host`
//! builder, middleware assembly, route/direct-dispatch adaptation, JWT
//! auth/authorization, and production middleware (compression, timing, tracing,
//! rate limiting).

pub mod auth;
pub mod auth_jwt;
pub mod authz;
pub mod compression;
pub mod cors;
pub mod diagnostics;
pub mod endpoint;
pub mod health;
pub mod memory_cache;
pub mod metrics;
pub mod options_watch;
#[cfg(feature = "password-argon2")]
pub mod password_argon2;
pub mod pipeline;
pub mod problem_response;
pub mod rate_limit;
pub mod request_id;
pub mod request_tracing;
pub mod router;
pub mod security_headers;
pub mod server;
pub mod timing;

#[cfg(feature = "testing")]
pub mod testing;

pub use auth::*;
pub use auth_jwt::*;
pub use authz::*;
pub use compression::*;
pub use cors::*;
pub use diagnostics::*;
pub use endpoint::*;
pub use health::*;
pub use memory_cache::*;
pub use metrics::*;
pub use options_watch::*;
#[cfg(feature = "password-argon2")]
pub use password_argon2::*;
pub use pipeline::*;
pub use problem_response::*;
pub use rate_limit::*;
pub use request_id::*;
pub use request_tracing::*;
pub use router::*;
pub use security_headers::*;
pub use server::*;
pub use timing::*;
