//! HTTP server, middleware pipeline, and routing (package `rust-webx-host`,
//! imported as `webx_host`).
//!
//! Implements the [`webx_core`] contracts on the 2.0 worker engine: the `Host`
//! builder, middleware assembly, route/direct-dispatch adaptation, JWT
//! auth/authorization, and production middleware (compression, timing, tracing,
//! rate limiting).

pub mod advisory_lock;
pub mod alt_svc;
pub mod auth;
pub mod auth_basic;
pub mod auth_cookie;
pub mod auth_dpop;
pub mod auth_jwks;
pub mod auth_jwt;
pub mod auth_mtls;
pub mod authz;
pub mod client_info;
pub mod compression;
pub mod config_validation;
pub mod cors;
pub mod csrf;
pub mod diagnostics;
pub mod endpoint;
pub mod health;
pub mod idempotency;
pub mod introspection;
pub mod login;
pub mod memory_cache;
pub mod metrics;
pub mod notifications;
pub mod options_watch;
#[cfg(feature = "password-argon2")]
pub mod password_argon2;
pub mod pipeline;
pub mod problem_response;
pub mod rate_limit;
pub mod realtime_fanout;
pub mod refresh;
pub mod request_id;
pub mod request_tracing;
pub mod router;
pub mod secret_protector;
pub mod security_headers;
pub mod server;
pub mod session_cache;
pub mod timing;
pub mod websocket;

pub mod testing;

pub use advisory_lock::*;
pub use alt_svc::*;
pub use auth::*;
pub use auth_basic::*;
pub use auth_cookie::*;
pub use auth_dpop::*;
pub use auth_jwks::*;
pub use auth_jwt::*;
pub use auth_mtls::*;
pub use authz::*;
pub use client_info::*;
pub use compression::*;
pub use config_validation::*;
pub use cors::*;
pub use csrf::*;
pub use diagnostics::*;
pub use endpoint::*;
pub use health::*;
pub use idempotency::*;
pub use introspection::*;
pub use login::*;
pub use memory_cache::*;
pub use metrics::*;
pub use notifications::*;
pub use options_watch::*;
#[cfg(feature = "password-argon2")]
pub use password_argon2::*;
pub use pipeline::*;
pub use problem_response::*;
pub use rate_limit::*;
pub use refresh::*;
pub use request_id::*;
pub use request_tracing::*;
pub use router::*;
pub use secret_protector::*;
pub use security_headers::*;
pub use server::*;
pub use session_cache::*;
pub use timing::*;
