//! HTTP/3 transport configuration.
//!
//! This is deliberately a plain value object: the host owns configuration
//! sources (`appsettings.json`, `APP__` environment overrides, `AppOptions`), so
//! the adapter only documents the exact knobs it needs and keeps parsing of TLS
//! material in [`crate::H3Transport::new`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use webx_core::config::TlsClientAuth;

use crate::webtransport::IWebTransportHandler;

/// Default per-request body cap: 8 MiB.
///
/// HTTP/3 requests are buffered by the neutral engine port, so this is the
/// memory a single request can force the transport to allocate. Uploads above
/// the cap are answered with `413 application/problem+json` and the request is
/// not dispatched.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Default QUIC keep-alive interval: frequent enough to keep long-lived
/// WebSocket/WebTransport sessions alive through the default idle timeout.
pub const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Default QUIC idle timeout: long-lived realtime sessions rely on the
/// keep-alive above; ordinary requests never wait this long.
pub const DEFAULT_MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Default concurrent bidirectional streams per QUIC connection (bounded to
/// keep the buffered-body worst case manageable: 16 × 8 MiB).
pub const DEFAULT_MAX_CONCURRENT_BIDI_STREAMS: u32 = 16;

/// Default concurrent unidirectional streams per QUIC connection.
pub const DEFAULT_MAX_CONCURRENT_UNI_STREAMS: u32 = 16;

/// Configuration for an [`H3Transport`](crate::H3Transport).
///
/// ```ignore
/// use webx_h3::H3Config;
///
/// let config = H3Config::new("0.0.0.0:4433", "certs/cert.pem", "certs/key.pem")
///     .with_max_request_body_bytes(2 * 1024 * 1024);
/// ```
#[non_exhaustive]
#[derive(Clone)]
pub struct H3Config {
    /// UDP address to bind, in `IP:port` form.
    ///
    /// Port `0` asks the operating system for a free port; the chosen address is
    /// reported by [`H3Transport::local_addr`](crate::H3Transport::local_addr).
    /// Host names are not resolved by the adapter: configure an IP address, as
    /// QUIC endpoints must bind to a concrete local socket.
    pub bind: String,
    /// PEM certificate chain served for the TLS 1.3 handshake.
    pub cert_path: PathBuf,
    /// PEM private key paired with [`Self::cert_path`].
    ///
    /// PKCS#8 (`PRIVATE KEY`), PKCS#1 (`RSA PRIVATE KEY`) and SEC1
    /// (`EC PRIVATE KEY`) blocks are accepted.
    pub key_path: PathBuf,
    /// Maximum number of request-body bytes buffered per request.
    ///
    /// Defaults to [`DEFAULT_MAX_REQUEST_BODY_BYTES`]. Requests carrying more
    /// bytes are answered with a `413` problem response and never reach routing.
    pub max_request_body_bytes: usize,
    /// Client-certificate policy for the QUIC/TLS handshake.
    ///
    /// `Off` (the default) uses `with_no_client_auth()`. `Optional`/`Required`
    /// verify the peer chain against [`Self::client_ca_path`]; `Required`
    /// rejects a connection that presents no certificate.
    pub client_auth: TlsClientAuth,
    /// PEM CA bundle used to verify client certificates when
    /// [`Self::client_auth`] is not `Off`.
    ///
    /// The CA file is only read at startup; an empty/invalid path is a readable
    /// configuration error instead of a plaintext/mTLS downgrade.
    pub client_ca_path: PathBuf,
    /// Optional WebTransport handler (adapter-level MVP; see
    /// [`crate::webtransport`]).
    ///
    /// When set, the HTTP/3 connection advertises WebTransport + datagrams and
    /// adopts RFC 9220 extended CONNECT sessions for this handler. When `None`,
    /// WebTransport requests are answered `501` and the transport does not
    /// advertise WebTransport support.
    pub webtransport_handler: Option<Arc<dyn IWebTransportHandler>>,
    /// QUIC keep-alive interval; `None` disables keep-alive.
    ///
    /// Defaults to [`DEFAULT_KEEP_ALIVE_INTERVAL`]. Set this below
    /// [`Self::max_idle_timeout`] so idle WebSocket/WebTransport sessions do
    /// not hit quinn's idle timer.
    pub keep_alive_interval: Option<Duration>,
    /// QUIC idle timeout; `None` leaves quinn's default (30 s).
    ///
    /// Defaults to [`DEFAULT_MAX_IDLE_TIMEOUT`].
    pub max_idle_timeout: Option<Duration>,
    /// Maximum concurrent bidirectional streams per QUIC connection.
    ///
    /// Defaults to [`DEFAULT_MAX_CONCURRENT_BIDI_STREAMS`]. Each concurrent
    /// request may buffer up to [`Self::max_request_body_bytes`] in the adapter.
    pub max_concurrent_bidi_streams: u32,
    /// Maximum concurrent unidirectional streams per QUIC connection.
    ///
    /// Defaults to [`DEFAULT_MAX_CONCURRENT_UNI_STREAMS`].
    pub max_concurrent_uni_streams: u32,
}

impl std::fmt::Debug for H3Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("H3Config")
            .field("bind", &self.bind)
            .field("cert_path", &self.cert_path)
            .field("key_path", &self.key_path)
            .field("max_request_body_bytes", &self.max_request_body_bytes)
            .field("client_auth", &self.client_auth)
            .field("client_ca_path", &self.client_ca_path)
            .field(
                "webtransport_handler",
                &self.webtransport_handler.as_ref().map(|_| "configured"),
            )
            .field("keep_alive_interval", &self.keep_alive_interval)
            .field("max_idle_timeout", &self.max_idle_timeout)
            .field(
                "max_concurrent_bidi_streams",
                &self.max_concurrent_bidi_streams,
            )
            .field(
                "max_concurrent_uni_streams",
                &self.max_concurrent_uni_streams,
            )
            .finish()
    }
}

impl H3Config {
    /// A configuration with the default 8 MiB request-body cap.
    pub fn new(
        bind: impl Into<String>,
        cert_path: impl Into<PathBuf>,
        key_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bind: bind.into(),
            cert_path: cert_path.into(),
            key_path: key_path.into(),
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            client_auth: TlsClientAuth::Off,
            client_ca_path: PathBuf::new(),
            webtransport_handler: None,
            keep_alive_interval: Some(DEFAULT_KEEP_ALIVE_INTERVAL),
            max_idle_timeout: Some(DEFAULT_MAX_IDLE_TIMEOUT),
            max_concurrent_bidi_streams: DEFAULT_MAX_CONCURRENT_BIDI_STREAMS,
            max_concurrent_uni_streams: DEFAULT_MAX_CONCURRENT_UNI_STREAMS,
        }
    }

    /// Require or optionally verify client certificates against a PEM CA bundle.
    ///
    /// ```ignore
    /// let config = H3Config::new("0.0.0.0:443", "cert.pem", "key.pem")
    ///     .with_client_auth("certs/client-ca.pem", TlsClientAuth::Required);
    /// ```
    #[must_use]
    pub fn with_client_auth(
        mut self,
        client_ca_path: impl Into<PathBuf>,
        client_auth: TlsClientAuth,
    ) -> Self {
        self.client_ca_path = client_ca_path.into();
        self.client_auth = client_auth;
        self
    }

    /// Attach the WebTransport session handler.
    ///
    /// This is an adapter-level MVP port (see [`crate::webtransport`]); it does
    /// not change the application-facing `webx` API. Without a handler the
    /// transport neither advertises nor accepts WebTransport sessions.
    #[must_use]
    pub fn with_webtransport_handler(mut self, handler: Arc<dyn IWebTransportHandler>) -> Self {
        self.webtransport_handler = Some(handler);
        self
    }

    /// Replace the request-body cap (in bytes).
    #[must_use]
    pub fn with_max_request_body_bytes(mut self, max_request_body_bytes: usize) -> Self {
        self.max_request_body_bytes = max_request_body_bytes;
        self
    }

    /// Set the QUIC keep-alive interval (`None` disables keep-alive).
    #[must_use]
    pub fn with_keep_alive_interval(mut self, interval: Option<Duration>) -> Self {
        self.keep_alive_interval = interval;
        self
    }

    /// Set the QUIC idle timeout (`None` keeps quinn's default).
    #[must_use]
    pub fn with_max_idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.max_idle_timeout = timeout;
        self
    }

    /// Set the per-connection concurrent stream limits (clamped to at least 1).
    #[must_use]
    pub fn with_stream_limits(
        mut self,
        max_concurrent_bidi_streams: u32,
        max_concurrent_uni_streams: u32,
    ) -> Self {
        self.max_concurrent_bidi_streams = max_concurrent_bidi_streams.max(1);
        self.max_concurrent_uni_streams = max_concurrent_uni_streams.max(1);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_the_default_body_cap_and_keeps_paths() {
        let config = H3Config::new("127.0.0.1:0", "cert.pem", "key.pem");
        assert_eq!(config.bind, "127.0.0.1:0");
        assert_eq!(config.cert_path, PathBuf::from("cert.pem"));
        assert_eq!(config.key_path, PathBuf::from("key.pem"));
        assert_eq!(
            config.max_request_body_bytes,
            DEFAULT_MAX_REQUEST_BODY_BYTES
        );
        assert_eq!(DEFAULT_MAX_REQUEST_BODY_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn the_body_cap_is_overridable() {
        let config =
            H3Config::new("127.0.0.1:0", "cert.pem", "key.pem").with_max_request_body_bytes(1024);
        assert_eq!(config.max_request_body_bytes, 1024);
    }

    #[test]
    fn quic_limits_have_safe_defaults_and_are_configurable() {
        let config = H3Config::new("127.0.0.1:0", "cert.pem", "key.pem");
        assert_eq!(
            config.keep_alive_interval,
            Some(DEFAULT_KEEP_ALIVE_INTERVAL)
        );
        assert_eq!(config.max_idle_timeout, Some(DEFAULT_MAX_IDLE_TIMEOUT));
        assert_eq!(
            config.max_concurrent_bidi_streams,
            DEFAULT_MAX_CONCURRENT_BIDI_STREAMS
        );
        assert_eq!(
            config.max_concurrent_uni_streams,
            DEFAULT_MAX_CONCURRENT_UNI_STREAMS
        );

        let config = config
            .with_keep_alive_interval(None)
            .with_max_idle_timeout(Some(Duration::from_secs(120)))
            .with_stream_limits(0, 0);
        assert_eq!(config.keep_alive_interval, None);
        assert_eq!(config.max_idle_timeout, Some(Duration::from_secs(120)));
        assert_eq!(config.max_concurrent_bidi_streams, 1);
        assert_eq!(config.max_concurrent_uni_streams, 1);
    }

    #[test]
    fn client_auth_defaults_off_and_is_configurable() {
        let config = H3Config::new("127.0.0.1:0", "cert.pem", "key.pem");
        assert_eq!(config.client_auth, TlsClientAuth::Off);
        assert!(config.client_ca_path.as_os_str().is_empty());

        let config = config.with_client_auth("ca.pem", TlsClientAuth::Required);
        assert_eq!(config.client_auth, TlsClientAuth::Required);
        assert_eq!(config.client_ca_path, PathBuf::from("ca.pem"));
    }
}
