//! Worker threads: one `current_thread` runtime each, fed by the acceptor over
//! a bounded channel.
//!
//! # Why threads
//!
//! A per-worker `current_thread` runtime avoids cross-core migration and keeps
//! per-connection state local. Each worker therefore owns:
//!
//! * a `current_thread` runtime (`enable_all` for the IO/time drivers);
//! * one hyper `auto::Builder` — configured once, cloned per connection because
//!   `serve_connection` borrows it and `into_owned` needs `Clone`;
//! * a `JoinSet` of live connections; global admission is a single `Semaphore`
//!   shared by every listener (its permit travels with the socket in `Handoff`,
//!   so it is released exactly when the connection task ends).
//!
//! # Handoff
//!
//! The acceptor never touches a worker's runtime: it accepts a
//! `tokio::net::TcpStream`, converts it to `std::net::TcpStream` with
//! `into_std()` (safe, deregisters from the acceptor reactor), and sends it over
//! an `mpsc` channel. The worker calls `TcpStream::from_std()` on its own
//! runtime, which registers the socket there. No `unsafe`, no `SO_REUSEPORT`
//! (unavailable on Windows and unnecessary — see the design doc).
//!
//! # Shutdown
//!
//! The worker loop observes the shared `watch`; it stops taking connections,
//! every live connection switches to `graceful_shutdown`, and the worker drains
//! its `JoinSet` for `drain_timeout` (or `stream_drain_timeout` while an SSE
//! response is active). Anything still running is aborted, so joining the
//! worker thread is bounded.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::DEFAULT_BLOCKING_THREADS;
use crate::pipeline::FinalHandler;
use crate::server::connection;
use crate::shared::{ActiveStreamGuard, ConnectionPermit, Shared};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use sha2::{Digest as _, Sha256};
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use webx_core::error::{Error, Result};

/// Per-worker handoff queue depth.
///
/// The acceptor is sequential (one `accept` at a time), so this only absorbs a
/// short burst while a worker is spawning connection tasks; it is not the
/// connection limit (that is the per-worker budget).
pub(crate) const WORKER_QUEUE_DEPTH: usize = 64;

/// TLS handshake deadline inside the worker.
///
/// A client that connects and stalls must not pin a connection-budget permit;
/// handshake failures drop the socket and release the permit.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Even a zero-configured drain gets one second to finish in-flight requests.
const MIN_DRAIN_BUDGET: Duration = Duration::from_secs(1);

/// A connection handed from an acceptor to a worker.
pub(crate) struct Handoff {
    /// Already in nonblocking mode; the worker registers it on its runtime.
    pub stream: std::net::TcpStream,
    pub peer: SocketAddr,
    /// TLS configuration for the listener this connection arrived on.
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// Global admission permit; dropped when the connection task ends.
    ///
    /// Wrapped in [`ConnectionPermit`] so the permit's lifetime also drives the
    /// `engine_connections_active` gauge.
    pub permit: ConnectionPermit,
    /// Queue gauge; dropped exactly once, whether the message is delivered,
    /// rejected, or dropped at shutdown.
    pub pending: PendingGuard,
}

/// RAII gauge for the number of connections queued to a worker.
///
/// The acceptor increments it when creating the handoff and the worker (or a
/// failed/dropped send) drops it. Keeping the decrement in `Drop` removes the
/// send/recv double-accounting race that a pair of manual decrements had.
pub(crate) struct PendingGuard {
    counter: Arc<AtomicUsize>,
}

impl PendingGuard {
    pub(crate) fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        decrement(&self.counter);
    }
}

/// The acceptor-facing half of a worker; cheap to clone per listener.
#[derive(Clone)]
pub(crate) struct WorkerFeed {
    pub index: usize,
    pub tx: mpsc::Sender<Handoff>,
    /// Connections currently being served.
    pub active: Arc<AtomicUsize>,
    /// Connections queued in `tx` but not yet picked up.
    pub pending: Arc<AtomicUsize>,
}

/// One worker thread plus the handles the acceptor needs to feed it.
pub(crate) struct Worker {
    pub feed: WorkerFeed,
    join: std::thread::JoinHandle<()>,
}

impl Worker {
    /// Join the thread, reporting a panic as an operator-facing message.
    pub(crate) fn join(self) -> std::result::Result<(), String> {
        self.join
            .join()
            .map_err(|_| format!("engine worker {} panicked", self.feed.index))
    }
}

/// How the acceptor reports "my runtime is live" / "startup failed".
pub(crate) type ReadySender = mpsc::UnboundedSender<std::result::Result<(), String>>;

/// Spawn one worker thread and hand back its channel/gates.
pub(crate) fn spawn(index: usize, shared: Arc<Shared>, ready: ReadySender) -> Result<Worker> {
    let (tx, rx) = mpsc::channel(WORKER_QUEUE_DEPTH);
    let active = Arc::new(AtomicUsize::new(0));
    let pending = Arc::new(AtomicUsize::new(0));
    let feed = WorkerFeed {
        index,
        tx,
        active: Arc::clone(&active),
        pending: Arc::clone(&pending),
    };

    let thread_shared = Arc::clone(&shared);
    let join = std::thread::Builder::new()
        .name(format!("webx-worker-{index}"))
        .spawn(move || {
            worker_main(index, thread_shared, rx, active, pending, ready);
        })
        .map_err(|err| Error::Internal(format!("failed to spawn engine worker {index}: {err}")))?;

    Ok(Worker { feed, join })
}

fn worker_main(
    index: usize,
    shared: Arc<Shared>,
    rx: mpsc::Receiver<Handoff>,
    active: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
    ready: ReadySender,
) {
    let runtime = match Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(DEFAULT_BLOCKING_THREADS)
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = ready.send(Err(format!(
                "engine worker {index} could not build a current-thread runtime: {err}"
            )));
            return;
        }
    };

    // Report only after the runtime exists: `Server::run` must not start
    // accepting into a channel whose consumer will never run.
    let _ = ready.send(Ok(()));

    runtime.block_on(worker_loop(index, shared, rx, active, pending));
    // Dropping the runtime here (after drain) releases the IO/time drivers; no
    // connection task survives `worker_loop`.
}

async fn worker_loop(
    index: usize,
    shared: Arc<Shared>,
    mut rx: mpsc::Receiver<Handoff>,
    active: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
) {
    let mut connections: JoinSet<()> = JoinSet::new();
    let mut shutdown = shared.subscribe_shutdown();
    // Stream accounting is per worker: one SSE connection in another worker
    // must not force this worker's normal connections to wait the 300s stream
    // drain budget.
    let worker_streams = Arc::new(AtomicUsize::new(0));
    // Built once per worker; cloned per connection because hyper's
    // `serve_connection` borrows the builder and `into_owned` requires `Clone`.
    let terminal: FinalHandler = connection::final_handler(Arc::clone(&shared));

    let builder = connection_builder(&shared);

    loop {
        tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown) => break,
            received = rx.recv() => {
                let Some(handoff) = received else { break };
                let Handoff {
                    stream,
                    peer,
                    tls,
                    permit,
                    pending: pending_guard,
                } = handoff;
                // Pending -> active transition: the guard is dropped exactly
                // once and the active guard owns the opposite transition.
                drop(pending_guard);
                // One sweep per accepted connection keeps the JoinSet small
                // without a separate reaper task.
                while connections.try_join_next().is_some() {}

                let connection_shared = Arc::clone(&shared);
                let connection_terminal = terminal.clone();
                let connection_builder = builder.clone();
                let active_guard = ActiveGuard::new(Arc::clone(&active));
                let connection_streams = Arc::clone(&worker_streams);
                connections.spawn(async move {
                    let _permit = permit;
                    let _active = active_guard;
                    run_connection(
                        connection_shared,
                        connection_terminal,
                        tls,
                        connection_builder,
                        stream,
                        peer,
                        connection_streams,
                    )
                    .await;
                });
            }
        }
    }

    // Stop intake; queued handoffs are dropped, closing their sockets and
    // releasing both the global permit and the pending gauge.
    drop(rx);
    let _ = pending;
    let label = format!("engine worker {index}");
    drain_connections(&mut connections, &label, shared.as_ref(), &worker_streams).await;
}

/// RAII gauge for connections currently being served by one worker.
struct ActiveGuard {
    counter: Arc<AtomicUsize>,
}

impl ActiveGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        decrement(&self.counter);
    }
}

/// Serve one socket until it closes or the server shuts down.
async fn run_connection(
    shared: Arc<Shared>,
    terminal: FinalHandler,
    tls: Option<Arc<rustls::ServerConfig>>,
    builder: auto::Builder<TokioExecutor>,
    stream: std::net::TcpStream,
    peer: SocketAddr,
    worker_streams: Arc<AtomicUsize>,
) {
    if let Err(err) = stream.set_nonblocking(true) {
        tracing::debug!(peer = %peer, error = %err, "engine: dropped connection (nonblocking mode)");
        return;
    }
    let stream = match tokio::net::TcpStream::from_std(stream) {
        Ok(stream) => stream,
        Err(err) => {
            tracing::debug!(peer = %peer, error = %err, "engine: dropped connection (runtime registration)");
            return;
        }
    };

    let shutdown = shared.subscribe_shutdown();
    let streams = Arc::new(ActiveStreamGuard::new(worker_streams));

    match tls {
        Some(config) => {
            let acceptor = TlsAcceptor::from(config);
            let handshake = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream));
            let mut handshake_shutdown = shutdown.clone();
            let handshake = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut handshake_shutdown) => return,
                result = handshake => result,
            };
            match handshake {
                Ok(Ok(tls_stream)) => {
                    let fingerprint = tls_stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .and_then(|certificates| certificates.first())
                        .map(|certificate| {
                            Arc::<str>::from(client_cert_fingerprint(certificate.as_ref()))
                        });
                    serve_io(
                        TokioIo::new(tls_stream),
                        shared,
                        terminal,
                        builder,
                        peer,
                        streams,
                        fingerprint,
                        shutdown,
                    )
                    .await;
                }
                Ok(Err(err)) => {
                    tracing::warn!(peer = %peer, error = %err, "engine: TLS handshake failed");
                }
                Err(_) => {
                    tracing::warn!(
                        peer = %peer,
                        timeout_secs = TLS_HANDSHAKE_TIMEOUT.as_secs(),
                        "engine: TLS handshake timed out"
                    );
                }
            }
        }
        None => {
            serve_io(
                TokioIo::new(stream),
                shared,
                terminal,
                builder,
                peer,
                streams,
                None,
                shutdown,
            )
            .await;
        }
    }
}

/// Build the hyper auto (HTTP/1.1 + HTTP/2) server builder for this engine.
///
/// One builder is built per worker and cloned per connection. The in-process
/// [`serve_stream`] path uses the same configuration, so a wire test exercises
/// the production protocol settings (header read timeout, header buffer bound,
/// HTTP/2 keepalive) instead of hyper defaults.
pub(crate) fn connection_builder(shared: &Shared) -> auto::Builder<TokioExecutor> {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(shared.config.header_read_timeout)
        .max_buf_size(shared.config.max_header_buf);
    builder
        .http2()
        .timer(TokioTimer::new())
        .keep_alive_interval(Some(shared.config.header_read_timeout))
        .keep_alive_timeout(shared.config.header_read_timeout);
    builder
}

/// Serve one already-connected transport through the engine pipeline.
///
/// This is the transport-neutral entry point used by the in-process wire test
/// harness and by custom transport adapters: the caller owns accept and TLS and
/// passes any `AsyncRead + AsyncWrite` stream. It runs the same hyper connection
/// loop, router, middleware, error writers and graceful drain handling as a TCP
/// connection, but without a socket, admission slot or TLS handshake. The
/// returned future completes when the connection closes or a graceful shutdown
/// drains it.
pub(crate) async fn serve_stream<I>(shared: Arc<Shared>, io: I, peer: SocketAddr)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let terminal: FinalHandler = connection::final_handler(Arc::clone(&shared));
    let builder = connection_builder(&shared);
    // Standalone stream accounting keeps the SSE drain budget meaningful for
    // this connection without joining a worker's per-worker gauge.
    let streams = Arc::new(ActiveStreamGuard::new(Arc::new(AtomicUsize::new(0))));
    let shutdown = shared.subscribe_shutdown();
    serve_io(
        TokioIo::new(io),
        shared,
        terminal,
        builder,
        peer,
        streams,
        None,
        shutdown,
    )
    .await;
}

/// Drive one hyper connection over a plain or TLS transport.
#[allow(clippy::too_many_arguments)]
async fn serve_io<I>(
    io: I,
    shared: Arc<Shared>,
    terminal: FinalHandler,
    builder: auto::Builder<TokioExecutor>,
    peer: SocketAddr,
    streams: Arc<ActiveStreamGuard>,
    client_cert: Option<Arc<str>>,
    mut shutdown: watch::Receiver<bool>,
) where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let first_request_started = Arc::new(AtomicBool::new(false));
    let first_request_notify = Arc::new(tokio::sync::Notify::new());
    let service_first = Arc::clone(&first_request_started);
    let service_notify = Arc::clone(&first_request_notify);
    let timeout = shared.config.first_request_timeout;

    let service =
        hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
            // This is the transition from "connection admitted" to "request
            // processing". The first-request watchdog stops once this fires.
            service_first.store(true, Ordering::Release);
            service_notify.notify_one();
            let shared = Arc::clone(&shared);
            let terminal = terminal.clone();
            let streams = Arc::clone(&streams);
            let client_cert = client_cert.clone();
            // WebSocket upgrades are handed to the framework after the `101`
            // response; the engine only extracts hyper's upgrade future.
            let upgrade = is_websocket_upgrade(&request).then(|| hyper::upgrade::on(&mut request));
            async move {
                connection::serve(
                    shared,
                    request,
                    Some(peer),
                    streams,
                    client_cert,
                    terminal,
                    upgrade,
                )
                .await
            }
        });

    // Upgrade-aware connection: the future stays pending until upgraded IO
    // closes, so WebSocket sessions remain counted by drain/metrics.
    let conn = builder.serve_connection_with_upgrades(io, service);
    let mut conn = std::pin::pin!(conn);

    let mut first_wait = Box::pin(wait_for_first_request(
        Arc::clone(&first_request_started),
        Arc::clone(&first_request_notify),
        timeout,
    ));
    let mut first_done = first_request_started.load(Ordering::Acquire);

    loop {
        tokio::select! {
            result = conn.as_mut() => {
                if let Err(err) = result {
                    tracing::debug!(peer = %peer, error = %err, "engine: connection closed");
                }
                return;
            }
            _ = first_wait.as_mut(), if !first_done => {
                first_done = true;
                if !first_request_started.load(Ordering::Acquire) {
                    tracing::debug!(
                        peer = %peer,
                        timeout_secs = timeout.as_secs(),
                        "engine: closing connection before its first request"
                    );
                    return;
                }
            }
            _ = wait_for_shutdown(&mut shutdown) => {
                // Finish in-flight requests and answer with `Connection: close`
                // instead of aborting the socket; the drain budget bounds this.
                conn.as_mut().graceful_shutdown();
                if let Err(err) = conn.await {
                    tracing::debug!(peer = %peer, error = %err, "engine: connection closed during shutdown");
                }
                return;
            }
        }
    }
}

/// SHA-256 of the DER client certificate, lowercase hex.
fn client_cert_fingerprint(certificate: &[u8]) -> String {
    let digest = Sha256::digest(certificate);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether the client asked for a WebSocket protocol upgrade.
fn is_websocket_upgrade(request: &hyper::Request<hyper::body::Incoming>) -> bool {
    request
        .headers()
        .get(hyper::header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Wait until the first request starts, or the pre-request deadline expires.
async fn wait_for_first_request(
    started: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    timeout: Duration,
) {
    if started.load(Ordering::Acquire) {
        return;
    }
    tokio::select! {
        _ = notify.notified() => {}
        _ = tokio::time::sleep(timeout) => {}
    }
}

/// Drain the worker's live connections within the shutdown budget.
async fn drain_connections(
    connections: &mut JoinSet<()>,
    label: &str,
    shared: &Shared,
    worker_streams: &AtomicUsize,
) {
    if shared.immediate_shutdown.load(Ordering::Acquire) {
        tracing::warn!(label, "engine run was abandoned; aborting connections");
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        return;
    }

    let budget = drain_timeout_for(
        shared.config.drain_timeout,
        shared.config.stream_drain_timeout,
        worker_streams.load(Ordering::Relaxed),
    );
    let mut immediate = shared.immediate_watch.subscribe();

    let drain = async {
        loop {
            tokio::select! {
                joined = connections.join_next() => match joined {
                    Some(Ok(())) => {}
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "engine: connection task ended abnormally");
                    }
                    None => break,
                },
                _ = immediate.wait_for(|value| *value) => {
                    tracing::warn!(label, "engine run was abandoned; aborting connections");
                    connections.abort_all();
                    while connections.join_next().await.is_some() {}
                    return;
                }
            }
        }
        // HTTP protocol upgrades detach the session task from the connection
        // future after the handoff; wait for those sessions so a graceful-close
        // frame can flush before the worker runtime is dropped. An abandoned run
        // (`immediate_watch`) must not wait out the whole drain budget for them.
        let mut upgrades = shared.admission.subscribe_upgrade_sessions();
        if *upgrades.borrow() > 0 {
            tokio::select! {
                _ = upgrades.wait_for(|count| *count == 0) => {}
                _ = immediate.wait_for(|value| *value) => {
                    tracing::warn!(label, "engine run was abandoned; dropping upgrade sessions");
                }
            }
        }
    };

    match tokio::time::timeout(budget, drain).await {
        Ok(()) => tracing::debug!(label, "engine worker drained"),
        Err(_) => {
            tracing::warn!(label, "engine worker drain timed out; aborting connections");
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        }
    }
}

/// Drain budget for one worker.
///
/// The worker-local SSE gauge selects the stream budget, so one streaming
/// connection in another worker cannot force this worker's ordinary
/// connections to wait the long budget. A configured zero is clamped to one
/// second.
pub(crate) fn drain_timeout_for(
    normal: Duration,
    streaming: Duration,
    active_streams: usize,
) -> Duration {
    let chosen = if active_streams > 0 {
        streaming
    } else {
        normal
    };
    chosen.max(MIN_DRAIN_BUDGET)
}

/// Wait until shutdown is requested (or the sender disappears).
pub(crate) async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    loop {
        if shutdown.changed().await.is_err() {
            return;
        }
        if *shutdown.borrow() {
            return;
        }
    }
}

pub(crate) fn decrement(counter: &AtomicUsize) {
    // Saturating CAS: no wrap on a buggy double-decrement, and no dependence on
    // the `fetch_update` method that newer toolchains deprecate.
    let mut current = counter.load(Ordering::Relaxed);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_connections_get_the_stream_budget() {
        let normal = Duration::from_secs(30);
        let streaming = Duration::from_secs(300);
        assert_eq!(drain_timeout_for(normal, streaming, 0), normal);
        assert_eq!(drain_timeout_for(normal, streaming, 1), streaming);
    }

    #[test]
    fn zero_drain_budget_is_clamped_to_one_second() {
        assert_eq!(
            drain_timeout_for(Duration::ZERO, Duration::ZERO, 0),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn counters_saturate_instead_of_wrapping() {
        let counter = AtomicUsize::new(0);
        decrement(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        counter.fetch_add(1, Ordering::Relaxed);
        counter.fetch_add(1, Ordering::Relaxed);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        decrement(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
