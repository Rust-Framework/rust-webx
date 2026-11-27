//! Outbound HTTP client for rust-webx.
//!
//! The server side of rust-webx never needed an HTTP client; an AI gateway (or
//! any service that calls other services) does. This crate provides one,
//! deliberately small surface:
//!
//! * [`HttpClient`] — a pool-backed client with timeouts and streaming
//!   responses ([`ResponseStream`]).
//! * [`IHttpClient`] — the port applications depend on (tests can substitute a
//!   mock without opening sockets by returning
//!   [`OutboundResponse::from_bytes`]).
//! * [`HttpClientRegistry`] / [`IHttpClientFactory`] — named clients such as
//!   `"openai"` and `"anthropic"`.
//! * [`SseStream`] — decodes an upstream `text/event-stream` body without
//!   buffering the whole response.
//!
//! Scope rules:
//!
//! * one HTTP stack (`hyper` + `hyper-util` + `hyper-rustls`), not a second one;
//! * request bodies are bytes for now — streaming uploads are a follow-up;
//! * retries/circuit breaking are a follow-up (`rust-webx-client` exposes the
//!   typed [`HttpClientError::is_connect`] signal they need).
//!
//! ```ignore
//! let client = HttpClient::new("openai", HttpClientOptions::new("https://api.openai.com"))?;
//! let response = client
//!     .execute(OutboundRequest::post("/v1/chat/completions").json(&payload)?)
//!     .await?;
//! let mut events = response.body.sse();
//! while let Some(event) = events.next().await {
//!     let event = event?;
//!     // forward `event.data` to the downstream SSE response
//! }
//! ```

mod client;
mod error;
mod registry;
mod sse;
pub mod testing;

pub use client::{
    BodySource, HttpClient, HttpClientOptions, IHttpClient, OutboundRequest, OutboundResponse,
    ResponseStream,
};
pub use error::HttpClientError;
pub use registry::{HttpClientRegistry, IHttpClientFactory};
pub use sse::{SseStream, DEFAULT_MAX_BUFFER};
