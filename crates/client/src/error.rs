//! Typed errors for outbound HTTP.

use std::time::Duration;
use thiserror::Error;

/// A failure while sending a request or reading its response.
///
/// Non-2xx responses are **not** errors: [`OutboundResponse`](crate::OutboundResponse)
/// carries the status so the caller decides how to map it (pass through, fail
/// over to another provider, or translate to a gateway error).
#[derive(Debug, Error)]
pub enum HttpClientError {
    /// The request itself is invalid (bad header name/value, bad JSON body).
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The configured base URL or the resolved request URL is not a valid URI.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// The connection could not be established (DNS, TCP, TLS handshake).
    #[error("connect failed: {0}")]
    Connect(String),

    /// A request or body-read deadline elapsed.
    #[error("request timed out after {0:?}")]
    Timeout(Duration),

    /// The connection was established but the HTTP exchange failed.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// SSRF protection rejected a private/link-local/metadata destination.
    #[error("blocked outbound destination: {0}")]
    BlockedAddress(String),

    /// Reading the response body failed after the headers arrived.
    #[error("response body error: {0}")]
    Body(String),

    /// The response body exceeded the caller's collection limit.
    #[error("response body exceeded the {limit}-byte limit")]
    BodyTooLarge { limit: usize },

    /// `IHttpClientFactory::create` was asked for a name that is not registered.
    #[error("unknown HTTP client: {0}")]
    UnknownClient(String),
}

impl HttpClientError {
    /// Whether retrying may succeed: connection failures and deadlines are
    /// transient, request/URL/protocol errors usually are not.
    ///
    /// Retry code must still respect the "no retry after the first byte" rule
    /// for streaming responses.
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Connect(_) | Self::Timeout(_))
    }
}
