//! rust-webx 2.0 execution engine.
//!
//! From-scratch HTTP execution layer: one acceptor, N current-thread worker
//! runtimes, zero-copy request context, direct handler dispatch and an onion
//! middleware pipeline. Application-facing contracts live in `webx_core`.

pub mod body;
pub mod config;
pub mod context;
mod hyper_compat;
pub mod pipeline;
pub mod response;
pub mod router;
pub mod server;
pub mod shared;
pub mod tls;
pub mod transport;
pub mod worker;

pub use body::BodyLimits;
pub use config::{EngineConfig, TlsConfig};
pub use context::EngineContext;
pub use pipeline::{FinalHandler, Pipeline};
pub use response::{EndpointResult, RespBody};
pub use router::{DirectDispatchFn, DirectTarget, EndpointTarget, RouteMatch, RouteTarget, Router};
pub use server::{Listener, Server, ServerHandle};
pub use shared::{AdmissionStats, ServerStats, Shared, TransportMetrics, TransportStats};
pub use transport::{
    IHttpTransport, TransportConnectionPermit, TransportRequest, TransportResponse,
    TransportRuntime,
};
