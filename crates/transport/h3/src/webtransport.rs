//! WebTransport (HTTP/3) handler port.
//!
//! This is an **adapter-level** MVP port. WebTransport sessions are established
//! with RFC 9220 extended CONNECT (`:protocol: webtransport`) and are handed to
//! an application-provided [`IWebTransportHandler`]. The session type is the
//! concrete `trev-h3-webtransport` server session over the pinned QUIC backend,
//! so applications can accept bidirectional/unidirectional streams and use the
//! datagram APIs directly.
//!
//! Unlike the neutral HTTP port used for ordinary requests, WebTransport streams
//! and datagrams cannot be losslessly represented by the engine's
//! `IUpgradeSession` byte stream (`AsyncRead + AsyncWrite`). A transport-neutral
//! WebTransport port therefore remains future work; this MVP keeps the h3
//! dependency's session type at the optional adapter boundary instead of adding
//! an h3-specific type to `webx`/`webx_core`.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use webx_core::error::Result;

/// A WebTransport server session over the pinned h3/quinn backend.
pub type WebTransportServerSession =
    h3_webtransport::server::WebTransportSession<h3_quinn::Connection, Bytes>;

/// Optional request metadata for a WebTransport CONNECT.
///
/// WebTransport sessions are accepted at the transport-adapter boundary and do
/// **not** pass through the engine's HTTP middleware pipeline, so the handler is
/// responsible for authentication/authorization. This metadata carries the
/// extended-CONNECT request URI/headers, the QUIC peer and the verified
/// client-certificate fingerprint (when mTLS is enabled).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct WebTransportRequestInfo {
    /// CONNECT request URI (includes path and query).
    pub uri: http::Uri,
    /// CONNECT method (always `CONNECT`).
    pub method: http::Method,
    /// CONNECT request headers.
    pub headers: http::HeaderMap,
    /// QUIC peer address.
    pub peer: SocketAddr,
    /// SHA-256 of the verified client certificate, if mTLS authenticated.
    pub client_cert_fingerprint: Option<Arc<str>>,
}

/// Application callback for one accepted WebTransport session.
///
/// The handler runs until the session ends; dropping the [`Arc`] session closes
/// it. Streams accepted from the session implement `futures_util::io::AsyncRead`
/// / `AsyncWrite`; datagram access is available through the session.
///
/// Because WebTransport CONNECT bypasses the engine's HTTP pipeline, the
/// handler must authorize the session using [`WebTransportRequestInfo`]
/// (headers/fingerprint) and enforce its own tenant/user rules. There is no
/// automatic `RequestUser`/pipeline identity for WebTransport sessions.
#[async_trait::async_trait]
pub trait IWebTransportHandler: Send + Sync {
    /// Authorize/deny the CONNECT **before** the transport sends `200`.
    ///
    /// The default accepts every session. Return an error to answer with
    /// `403 application/problem+json` and tear the CONNECT stream down without
    /// establishing a WebTransport session (no 200 is sent).
    async fn authorize(&self, _info: &WebTransportRequestInfo) -> Result<()> {
        Ok(())
    }

    /// Serve one accepted WebTransport session.
    async fn on_session(
        &self,
        info: WebTransportRequestInfo,
        session: Arc<WebTransportServerSession>,
    ) -> Result<()>;
}

/// True when `request` is an RFC 9220 extended CONNECT for WebTransport.
pub(crate) fn is_webtransport_request(request: &http::Request<()>) -> bool {
    h3_webtransport::server::is_webtransport_request(request)
}
