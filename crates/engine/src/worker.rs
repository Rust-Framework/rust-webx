//! Worker threads: one `current_thread` runtime each, fed by the acceptor over
//! a bounded channel.
//!
//! # Why threads
//!
//! The measured prototype in `docs/design/engine-2.0.md` reaches ~219k RPS with
//! 22 current-thread workers versus ~114k for one multi-thread runtime at the
//! same 32-connection load. Each worker therefore owns:
//!
//! * a `current_thread` runtime (`enable_all` for the IO/time drivers);
//! * one hyper `auto::Builder` — configured once, cloned per connection because
//!   `serve_connection` borrows it and `into_owned` needs `Clone`;
//! * a `JoinSet` of live connections and its own connection budget (a
//!   `Semaphore` whose permits are acquired by the acceptor and travel with the
//!   socket in `Handoff`, so a permit is released exactly when the connection
//!   task ends).
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use webx_core::error::{Error, Result};

use crate::config::DEFAULT_BLOCKING_THREADS;
use crate::pipeline::FinalHandler;
use crate::server::connection;
use crate::shared::{ActiveStreamGuard, Shared};

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

/// A connection handed from the acceptor to a worker.
pub(crate) struct Handoff {
    /// Already in nonblocking mode; the worker registers it on its runtime.
    pub stream: std::net::TcpStream,
    pub peer: SocketAddr,
    /// Connection-budget permit; dropped when the connection task ends.
    pub permit: OwnedSemaphorePermit,
}

/// One worker thread plus the handles the acceptor needs to feed it.
pub(crate) struct Worker {
    pub index: usize,
    pub tx: mpsc::Sender<Handoff>,
    /// Per-worker connection budget.
    pub budget: Arc<Semaphore>,
    /// Connections currently being served.
    pub active: Arc<AtomicUsize>,
    /// Connections queued in [`Worker::tx`] but not yet picked up.
    pub pending: Arc<AtomicUsize>,
    join: std::thread::JoinHandle<()>,
}

impl Worker {
    /// Join the thread, reporting a panic as an operator-facing message.
    pub(crate) fn join(self) -> std::result::Result<(), String> {
        self.join
            .join()
            .map_err(|_| format!("engine worker {} panicked", self.index))
    }
}

/// How the acceptor reports "my runtime is live" / "startup failed".
pub(crate) type ReadySender = mpsc::UnboundedSender<std::result::Result<(), String>>;

/// Per-worker connection budget derived from the documented global limit.
///
/// Integer division keeps `budget * workers <= max_connections`, so the
/// effective global cap never exceeds the configured `App.MaxConnections`; the
/// floor of 1 keeps the server usable when there are more workers than
/// connections.
pub(crate) fn connection_budget(max_connections: usize, workers: usize) -> usize {
    (max_connections / workers.max(1)).max(1)
}

/// Spawn one worker thread and hand back its channel/gates.
pub(crate) fn spawn(index: usize, shared: Arc<Shared>, ready: ReadySender) -> Result<Worker> {
    let budget = connection_budget(
        shared.config.max_connections,
        shared.config.resolved_workers(),
    );
    let (tx, rx) = mpsc::channel(WORKER_QUEUE_DEPTH);
    let budget_gate = Arc::new(Semaphore::new(budget));
    let active = Arc::new(AtomicUsize::new(0));
    let pending = Arc::new(AtomicUsize::new(0));

    let thread_shared = Arc::clone(&shared);
    let thread_active = Arc::clone(&active);
    let thread_pending = Arc::clone(&pending);
    let join = std::thread::Builder::new()
        .name(format!("webx-worker-{index}"))
        .spawn(move || {
            worker_main(
                index,
                thread_shared,
                rx,
                thread_active,
                thread_pending,
                ready,
            );
        })
        .map_err(|err| Error::Internal(format!("failed to spawn engine worker {index}: {err}")))?;

    Ok(Worker {
        index,
        tx,
        budget: budget_gate,
        active,
        pending,
        join,
    })
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
    // Built once per worker; cloned per connection because hyper's
    // `serve_connection` borrows the builder and `into_owned` requires `Clone`.
    let terminal: FinalHandler = connection::final_handler(Arc::clone(&shared));
    let tls = shared
        .tls
        .as_ref()
        .map(|config| TlsAcceptor::from(Arc::clone(config)));

    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(shared.config.header_read_timeout)
        .max_buf_size(shared.config.max_header_buf);
    builder.http2().timer(TokioTimer::new());

    loop {
        tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown) => break,
            received = rx.recv() => {
                let Some(handoff) = received else { break };
                decrement(&pending);
                // One sweep per accepted connection keeps the JoinSet small
                // without a separate reaper task.
                while connections.try_join_next().is_some() {}

                increment(&active);
                let connection_shared = Arc::clone(&shared);
                let connection_terminal = terminal.clone();
                let connection_tls = tls.clone();
                let connection_builder = builder.clone();
                let connection_active = Arc::clone(&active);
                connections.spawn(async move {
                    let _permit = handoff.permit;
                    run_connection(
                        connection_shared,
                        connection_terminal,
                        connection_tls,
                        connection_builder,
                        handoff.stream,
                        handoff.peer,
                    )
                    .await;
                    decrement(&connection_active);
                });
            }
        }
    }

    // Stop intake; queued handoffs are dropped, which closes their sockets and
    // releases their budget permits.
    drop(rx);
    let label = format!("engine worker {index}");
    drain_connections(&mut connections, &label, shared.as_ref()).await;
}

/// Serve one socket until it closes or the server shuts down.
async fn run_connection(
    shared: Arc<Shared>,
    terminal: FinalHandler,
    tls: Option<TlsAcceptor>,
    builder: auto::Builder<TokioExecutor>,
    stream: std::net::TcpStream,
    peer: SocketAddr,
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
    let streams = Arc::new(shared.active_stream_guard());

    match tls {
        Some(acceptor) => {
            let handshake =
                tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await;
            match handshake {
                Ok(Ok(tls_stream)) => {
                    serve_io(
                        TokioIo::new(tls_stream),
                        shared,
                        terminal,
                        builder,
                        peer,
                        streams,
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
                shutdown,
            )
            .await;
        }
    }
}

/// Drive one hyper connection over a plain or TLS transport.
async fn serve_io<I>(
    io: I,
    shared: Arc<Shared>,
    terminal: FinalHandler,
    builder: auto::Builder<TokioExecutor>,
    peer: SocketAddr,
    streams: Arc<ActiveStreamGuard>,
    mut shutdown: watch::Receiver<bool>,
) where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let service =
        hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
            let shared = Arc::clone(&shared);
            let terminal = terminal.clone();
            let streams = Arc::clone(&streams);
            async move { connection::serve(shared, request, Some(peer), streams, terminal).await }
        });

    // `into_owned` detaches the connection from the builder borrow, so the
    // connection future can be spawned/pinned independently.
    let conn = builder.serve_connection(io, service).into_owned();
    let mut conn = std::pin::pin!(conn);

    tokio::select! {
        result = conn.as_mut() => {
            if let Err(err) = result {
                tracing::debug!(peer = %peer, error = %err, "engine: connection closed");
            }
        }
        _ = wait_for_shutdown(&mut shutdown) => {
            // Finish in-flight requests and answer with `Connection: close`
            // instead of aborting the socket; the drain budget bounds this.
            conn.as_mut().graceful_shutdown();
            if let Err(err) = conn.await {
                tracing::debug!(peer = %peer, error = %err, "engine: connection closed during shutdown");
            }
        }
    }
}

/// Drain the worker's live connections within the shutdown budget.
async fn drain_connections(connections: &mut JoinSet<()>, label: &str, shared: &Shared) {
    let budget = drain_timeout_for(
        shared.config.drain_timeout,
        shared.config.stream_drain_timeout,
        shared.active_streams.load(Ordering::Relaxed),
    );

    let drain = async {
        while let Some(result) = connections.join_next().await {
            if let Err(err) = result {
                tracing::debug!(error = %err, "engine: connection task ended abnormally");
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
/// While any connection in the process serves an SSE response the stream budget
/// is used, so a live event stream can end cleanly instead of being cut off;
/// otherwise the normal drain budget applies. A configured zero is clamped to
/// one second.
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

pub(crate) fn increment(counter: &AtomicUsize) {
    counter.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn decrement(counter: &AtomicUsize) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        current.checked_sub(1)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_budget_never_exceeds_the_global_limit() {
        assert_eq!(connection_budget(10_000, 22), 454);
        assert_eq!(connection_budget(10_000, 1), 10_000);
        // More workers than connections still yields one slot each.
        assert_eq!(connection_budget(2, 8), 1);
    }

    #[test]
    fn connection_budget_floor_guards_a_zero_connection_limit() {
        assert_eq!(connection_budget(0, 4), 1);
        assert_eq!(connection_budget(4, 0), 4);
    }

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
        increment(&counter);
        increment(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        decrement(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
