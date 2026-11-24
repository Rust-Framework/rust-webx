//! Optional HTTP/3 listener transport for rust-webx.
//!
//! This crate is a **transport adapter**, not a second web framework: it owns
//! the UDP/QUIC endpoint, the TLS 1.3 handshake and HTTP/3 stream lifecycle and
//! hands every decoded request to the engine's neutral request port
//! ([`webx_engine::Server::serve_transport_request`]). Router, middleware,
//! request binding, error writers, panic boundary, security headers, file/SSE
//! finalization and DI therefore run the **production pipeline**; an application
//! keeps using the normal `webx` API and only opts in on the host:
//!
//! ```ignore
//! use std::sync::Arc;
//! use webx_h3::{H3Config, H3Transport};
//!
//! let transport = Arc::new(H3Transport::new(H3Config::new(
//!     "0.0.0.0:443",
//!     "certs/fullchain.pem",
//!     "certs/privkey.pem",
//! ))?);
//!
//! Host::builder()
//!     .use_http_transport(transport)
//!     .build()
//!     .run()
//!     .await?;
//! ```
//!
//! # What the adapter does
//!
//! * binds one [`quinn::Endpoint`] to the configured UDP address (port `0` asks
//!   the OS for a free port, [`H3Transport::local_addr`] reports the result);
//! * builds a TLS 1.3-only [`rustls::ServerConfig`] from PEM files with ALPN
//!   `h3` and converts it to QUIC via
//!   [`quinn::crypto::rustls::QuicServerConfig`];
//! * accepts QUIC connections, builds one `h3` server connection each and
//!   multiplexes requests on it (one spawned task per request stream);
//! * buffers each request body up to
//!   [`H3Config::max_request_body_bytes`](crate::H3Config) (8 MiB by default),
//!   answers `413 application/problem+json` above it, and forwards the verified
//!   client-certificate SHA-256 fingerprint to the engine so the
//!   `x-webx-client-cert-sha256` contract keeps exactly one writer (the TLS
//!   terminator);
//! * writes the engine's [`webx_engine::TransportResponse`] back as an HTTP/3
//!   response (headers minus hop-by-hop fields, `None`/`Bytes`/`Stream` bodies),
//!   so SSE and file/streaming responses are forwarded frame by frame;
//! * optionally verifies client certificates against a PEM CA bundle
//!   (`H3Config::with_client_auth`, `TlsClientAuth::Optional`/`Required`) and
//!   forwards the verified leaf SHA-256 to the engine, keeping exactly one
//!   writer for `x-webx-client-cert-sha256`;
//! * on engine shutdown sends a GOAWAY on every established connection, waits
//!   (bounded) for in-flight request tasks, then closes the endpoint and waits
//!   (bounded) for [`quinn::Endpoint::wait_idle`].
//!
//! # Scope and known limitations
//!
//! * **WebSocket over HTTP/3.** The transport enables RFC 9220 extended
//!   CONNECT and bridges `:protocol: websocket` requests to the engine's
//!   existing WebSocket middleware/`IUpgradeSession`: the h3 request stream is
//!   turned into a duplex byte stream, so the host handshake, authentication,
//!   identity, limits and observability are the same as over HTTP/1.1.
//!   A successful extended CONNECT is committed as HTTP `200` (not `101`);
//!   other extended-CONNECT protocols receive problem+json `501`.
//! * **WebTransport (adapter-level MVP).** With
//!   [`H3Config::with_webtransport_handler`](crate::H3Config), the transport
//!   advertises WebTransport + datagrams and adopts RFC 9220
//!   `:protocol: webtransport` sessions, handing a concrete
//!   [`WebTransportServerSession`]
//!   to the application handler. `IWebTransportHandler::authorize` runs before
//!   the session is accepted and is **fail-closed**: a handler that does not
//!   override it denies every session (no `200` is sent). Bidirectional stream
//!   echo is covered e2e;
//!   the session also exposes unidirectional streams and datagrams through the
//!   pinned h3 fork. A **transport-neutral** WebTransport port for
//!   `webx`/`webx_core` remains future work (streams/datagrams cannot be
//!   losslessly represented by `IUpgradeSession`'s byte stream). WebTransport
//!   sessions run inside the connection task; engine shutdown closes them by
//!   closing the QUIC connection after the drain budget (there is no per-session
//!   shutdown token yet).
//! * **Buffered request bodies.** The neutral port is buffered for the request
//!   body, so uploads above the cap are rejected with `413` instead of streamed.
//!   The cap is [`H3Config::max_request_body_bytes`](crate::H3Config) and is
//!   independent of `App.MaxBodySize`, so a large multipart upload that the
//!   HTTP/1.1+2 path accepts can be rejected over h3; raise the h3 cap when the
//!   application needs bigger uploads. The engine's first-request and request
//!   deadlines (`Server.FirstRequestTimeoutSeconds`/`RequestTimeoutSeconds`) are
//!   applied to every QUIC connection, so a silent or stalled client cannot pin
//!   an admission slot.
//! * **WebTransport types come from the pinned fork.** The handler signature
//!   exposes the fork's session/stream types; treat that adapter-level surface as
//!   experimental (the h3-owned configuration and transport API are the stable
//!   part).
//! * **Engine admission + per-transport metrics.** Every accepted QUIC
//!   connection takes one `App.MaxConnections` slot from the engine's global
//!   governor (`TransportRuntime::admit_connection`), so h1/h2 and h3
//!   connections share one budget. `Server::run_transport` registers this
//!   backend's counters, so `/metrics` exposes
//!   `engine_transport_*{transport="h3"}` (connections/requests/rejections/
//!   handshake failures). An over-capacity QUIC connection cannot receive a
//!   pre-handshake 503 and is closed after the admission deadline instead.
//! * **mTLS** is supported, including the verified-fingerprint bridge; it is
//!   configured explicitly through [`H3Config`] (no `AppOptions`/appsettings
//!   section yet).
//! * **0-RTT stays disabled.** QUIC early data is left off
//!   (`max_early_data_size = 0`) until the idempotency replay story is designed.
//! * **`Alt-Svc` is explicit.** Use
//!   `HostBuilder::use_h3_alt_svc(udp_port)` to advertise the endpoint on
//!   HTTP/1.1+2 responses; nothing is advertised unless configured.
//! * **h3 is a pinned fork.** `trev-h3 = 0.3.0` / `trev-h3-quinn = 0.3.0`
//!   (a fork of hyperium/h3 with RFC 9220 + WebTransport fixes) are pinned
//!   exactly and bridged through this crate's stable public surface
//!   ([`H3Config`]/[`H3Transport`]); upstream/fork breaking changes stay contained.
//!
//! # TLS material
//!
//! [`H3Transport::new`] reads the certificate chain (`cert_path`, one or more
//! PEM `CERTIFICATE` blocks) and the private key (`key_path`, PKCS#8, PKCS#1 or
//! SEC1 PEM) once at startup and reports unreadable, malformed or mismatched
//! material as [`webx_core::error::Error::Http`] without ever logging key bytes.
//! Certificates and keys are reload-on-restart, not hot-reloaded.

mod config;
mod tls;
mod transport;
pub mod webtransport;

pub use config::{
    H3Config, DEFAULT_KEEP_ALIVE_INTERVAL, DEFAULT_MAX_CONCURRENT_BIDI_STREAMS,
    DEFAULT_MAX_CONCURRENT_UNI_STREAMS, DEFAULT_MAX_IDLE_TIMEOUT, DEFAULT_MAX_REQUEST_BODY_BYTES,
};
pub use transport::H3Transport;
pub use webtransport::{IWebTransportHandler, WebTransportRequestInfo, WebTransportServerSession};

// Self-contained application surface: re-export the fork/adjacent types needed
// to implement `IWebTransportHandler` without adding direct dependencies on the
// pinned `trev-*` packages or on `rust-webx-core`.
pub use async_trait::async_trait;
pub use h3::ext::WebTransportDraft;
pub use h3_webtransport::server::AcceptedBi;
pub use h3_webtransport::stream::{BidiStream, RecvStream, SendStream};
pub use webx_core::config::TlsClientAuth;
