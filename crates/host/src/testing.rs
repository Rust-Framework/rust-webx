//! Test harnesses: in-process [`TestHost`] plus, behind the optional
//! `testing` feature, the ephemeral-port `TestServer` helpers.
//!
//! Prefer `TestHost` for handler/DI/event tests (no socket, no HTTP encoding);
//! use the port helpers only when the test must exercise routing, binding,
//! headers or the wire protocol byte-for-byte.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use webx_core::auth::IdentityUser;
use webx_core::error::{Error, Result};
use webx_core::mediator::{IMediator, IRequest, Mediator};
use webx_core::request_context::RequestContext;
use webx_core::route::scan::HandlerCache;
use webx_core::time::{IClock, TestClock};

use crate::server::{Host, HostBuilder};

/// Fault/latency injection for [`TestHost`] dispatch.
///
/// In-process faults are deterministic and explicit: a test arms `fail_next(n)`
/// or `delay_next(d)` and the next dispatches observe exactly that. HTTP-level
/// chaos (dropped connections, malformed frames) belongs to the wire-level
/// chaos middleware in the stability suite.
#[derive(Debug, Default)]
pub struct ChaosControl {
    fail_next: AtomicUsize,
    delay_next: Mutex<Option<Duration>>,
    dispatched: AtomicUsize,
}

impl ChaosControl {
    /// Make the next `count` dispatches fail with `503` before reaching the
    /// handler.
    pub fn fail_next(&self, count: usize) {
        self.fail_next.store(count, Ordering::SeqCst);
    }

    /// Delay the next dispatch by `delay`.
    pub fn delay_next(&self, delay: Duration) {
        *self
            .delay_next
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(delay);
    }

    /// Clear armed faults/latency.
    pub fn reset(&self) {
        self.fail_next.store(0, Ordering::SeqCst);
        self.delay_next
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    /// Number of dispatches attempted (including injected failures).
    pub fn dispatched(&self) -> usize {
        self.dispatched.load(Ordering::SeqCst)
    }

    async fn before_dispatch(&self) -> Result<()> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        let delay = self
            .delay_next
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mut remaining = self.fail_next.load(Ordering::SeqCst);
        while remaining > 0 {
            match self.fail_next.compare_exchange(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Err(Error::Status(
                        503,
                        "injected fault (TestHost chaos)".to_string(),
                    ))
                }
                Err(actual) => remaining = actual,
            }
        }
        Ok(())
    }
}

/// In-process host for dispatching typed requests without binding a port.
pub struct TestHost {
    host: Host,
    provider: Arc<rust_dix::ServiceProvider>,
    handler_cache: Arc<HandlerCache>,
    clock: Arc<dyn IClock>,
    chaos: Arc<ChaosControl>,
}

impl TestHost {
    /// Build a test host from a configured builder, with a [`TestClock`]
    /// starting at the current wall time.
    pub fn from_builder(builder: HostBuilder) -> Self {
        Self::from_builder_with_clock(builder, Arc::new(TestClock::default()))
    }

    /// Build a test host with an explicit controllable clock.
    pub fn from_builder_with_clock(builder: HostBuilder, clock: Arc<TestClock>) -> Self {
        let host = builder
            .use_clock(Arc::clone(&clock) as Arc<dyn IClock>)
            .build();
        let dispatch_runtime = host.dispatch_runtime();
        let provider = Arc::clone(dispatch_runtime.provider());
        let handler_cache = Arc::clone(dispatch_runtime.handler_cache());
        Self {
            host,
            provider,
            handler_cache,
            clock,
            chaos: Arc::new(ChaosControl::default()),
        }
    }

    /// Wrap an already-built host (uses the host's registered clock).
    pub fn new(host: Host) -> Self {
        let clock = host.clock();
        let dispatch_runtime = host.dispatch_runtime();
        let provider = Arc::clone(dispatch_runtime.provider());
        let handler_cache = Arc::clone(dispatch_runtime.handler_cache());
        Self {
            host,
            provider,
            handler_cache,
            clock,
            chaos: Arc::new(ChaosControl::default()),
        }
    }

    /// The underlying host (server, provider, dispatch runtime).
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The test host's clock (the same instance handlers inject).
    pub fn clock(&self) -> Arc<dyn IClock> {
        Arc::clone(&self.clock)
    }

    /// Fault/latency injection for [`Self::dispatch`].
    pub fn chaos(&self) -> Arc<ChaosControl> {
        Arc::clone(&self.chaos)
    }

    /// Dispatch an anonymous typed request through the mediator.
    pub async fn dispatch<T, R>(&self, request: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
    {
        self.dispatch_with(None, request).await
    }

    /// Dispatch a request with an authenticated identity in
    /// [`RequestContext`] (covered handlers can read `RequestContext::user_id`).
    pub async fn dispatch_as<T, R>(&self, identity: IdentityUser, request: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
    {
        self.dispatch_with(Some(Arc::new(identity)), request).await
    }

    async fn dispatch_with<T, R>(
        &self,
        identity: Option<Arc<IdentityUser>>,
        request: T,
    ) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
    {
        self.chaos.before_dispatch().await?;
        let mediator = Mediator::new(Arc::clone(&self.provider), Arc::clone(&self.handler_cache));
        RequestContext::run(identity, async move {
            IMediator::send::<T, R>(&mediator, request).await
        })
        .await
    }

    /// Shut down the host (stops hosted services and disposes the provider).
    pub fn shutdown(self) {
        self.host.server_handle().shutdown();
    }

    /// Open one in-process wire connection served by the real HTTP pipeline.
    ///
    /// The test gets the client end of a duplex pipe; the server end is driven
    /// by this host's engine (the same hyper connection loop, router,
    /// middleware, error writers and drain handling as a TCP listener) on the
    /// current runtime. No port is bound and no OS socket is involved, but
    /// protocol bytes are decoded by hyper exactly as in production: use this
    /// for keep-alive, h2c, SSE frame bytes and malformed-request tests that
    /// [`TestHost::dispatch`] (handler/DI level) cannot express.
    pub async fn connect_wire(&self) -> WireConnection {
        let (client, server) = tokio::io::duplex(WIRE_PIPE_BYTES);
        let engine = webx_engine::Server::new(self.host.engine_shared());
        // A loopback peer with an ephemeral-looking port; handlers observe it
        // through `ClientInfo` exactly as they would a real listener.
        let peer = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let task = tokio::spawn(async move {
            engine.serve_stream(server, peer).await;
        });
        WireConnection {
            stream: client,
            task,
        }
    }
    /// Serve one in-process neutral transport request through the real pipeline.
    ///
    /// The caller supplies a [`webx_engine::TransportRequest`] and receives the
    /// [`webx_engine::TransportResponse`]; router, middleware, handler dispatch,
    /// binding, error writer and panic boundary are the production ones. This is
    /// the request-level seam used by non-hyper transports (and by the
    /// transport-port tests).
    pub async fn serve_transport_request(
        &self,
        request: webx_engine::TransportRequest,
    ) -> webx_engine::TransportResponse {
        let engine = webx_engine::Server::new(self.host.engine_shared());
        engine.serve_transport_request(request).await
    }

    /// Engine-level shutdown handle for transport tests.
    ///
    /// A running `Host` bridges the host shutdown signal to the engine; a
    /// `TestHost` driven only through [`Self::serve_transport_request`] has no
    /// bridge, so this handle lets a test raise the engine shutdown directly.
    pub fn engine_handle(&self) -> webx_engine::ServerHandle {
        webx_engine::Server::new(self.host.engine_shared()).handle()
    }

    /// Run a listener-level transport backend through the real engine.
    ///
    /// The backend owns accept/handshake and uses the supplied
    /// [`webx_engine::TransportRuntime`] to submit requests through
    /// `Server::serve_transport_request`; this is the in-process seam used by
    /// the transport-port tests and by non-hyper adapters.
    pub async fn run_http_transport(
        &self,
        transport: Arc<dyn webx_engine::IHttpTransport>,
    ) -> Result<()> {
        let engine = webx_engine::Server::new(self.host.engine_shared());
        engine.run_transport(transport).await
    }
}
const WIRE_PIPE_BYTES: usize = 64 * 1024;

/// One in-process wire connection served by the real engine pipeline.
///
/// The client side is a duplex pipe into the host's engine; no TCP port is
/// bound. Drop or [`Self::close`] the connection when the test is done so the
/// server task is joined deterministically.
pub struct WireConnection {
    /// Client side of the pipe: raw HTTP/1.1 bytes or a hyper client transport.
    pub stream: tokio::io::DuplexStream,
    /// Server task; completes when the connection closes or drains.
    pub task: tokio::task::JoinHandle<()>,
}

impl WireConnection {
    /// Write raw request bytes.
    pub async fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.stream.write_all(bytes).await
    }

    /// Read at most `max` bytes; resolves when `max` is reached or the peer
    /// closes the pipe.
    pub async fn read_at_most(&mut self, max: usize) -> std::io::Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        let mut out = Vec::with_capacity(max.min(8 * 1024));
        let mut limited = (&mut self.stream).take(max as u64);
        limited.read_to_end(&mut out).await?;
        Ok(out)
    }

    /// Close the client end and wait for the server task to finish.
    pub async fn close(self) {
        drop(self.stream);
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chaos_fails_then_allows_dispatch_and_tracks_attempts() {
        let control = ChaosControl::default();
        control.fail_next(2);
        assert!(matches!(
            control.before_dispatch().await,
            Err(Error::Status(503, _))
        ));
        assert!(matches!(
            control.before_dispatch().await,
            Err(Error::Status(503, _))
        ));
        assert!(control.before_dispatch().await.is_ok());
        assert_eq!(control.dispatched(), 3);
        control.delay_next(Duration::from_millis(5));
        control.fail_next(1);
        assert!(matches!(
            control.before_dispatch().await,
            Err(Error::Status(503, _))
        ));
        assert!(control.before_dispatch().await.is_ok());
        control.reset();
        assert!(control.before_dispatch().await.is_ok());
    }
}

// ---------------------------------------------------------------------------
// Ephemeral-port helpers (`testing` feature)
// ---------------------------------------------------------------------------

/// Pick a free TCP port on localhost.
#[cfg(feature = "testing")]
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Poll until `GET {base}{health_path}` returns success.
#[cfg(feature = "testing")]
pub async fn wait_until_ready(base_url: &str, health_path: &str) {
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        health_path.trim_start_matches('/')
    );
    for _ in 0..60 {
        if let Ok(resp) = reqwest::get(&url).await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("server failed to become ready at {url}");
}

/// Running test server instance.
#[cfg(feature = "testing")]
pub struct TestServer {
    pub base_url: String,
    pub handle: crate::server::ServerHandle,
    pub task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "testing")]
impl TestServer {
    pub async fn teardown(self) {
        self.handle.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

/// Build host, bind to `127.0.0.1:{port}`, wait for health, return client base URL.
#[cfg(feature = "testing")]
pub async fn spawn(host: Host, port: u16) -> TestServer {
    spawn_with_health(host, port, "/health/live").await
}

/// Same as [`spawn`] with a custom readiness probe path.
#[cfg(feature = "testing")]
pub async fn spawn_with_health(host: Host, port: u16, health_path: &str) -> TestServer {
    let addr = format!("127.0.0.1:{port}");
    let base_url = format!("http://{addr}");
    let handle = host.server_handle();
    let task = tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });
    wait_until_ready(&base_url, health_path).await;
    TestServer {
        base_url,
        handle,
        task,
    }
}
