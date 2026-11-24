//! The engine server: one acceptor on the caller runtime, N worker threads,
//! graceful shutdown.
//!
//! # Topology
//!
//! ```text
//! caller runtime (acceptor task)
//!   TcpListener::accept()
//!     -> set_nodelay, TcpStream::into_std()
//!     -> least-loaded worker with capacity
//!     -> bounded mpsc  (per-worker connection budget permit travels with it)
//! worker thread (current_thread runtime)
//!   TcpStream::from_std()            // registers on the worker's reactor
//!   -> optional TLS accept (10s timeout, ALPN h2/http1.1)
//!   -> hyper auto::Builder per worker (into_owned per connection)
//!   -> EngineContext -> pipeline -> router terminal -> response
//! ```
//!
//! # Shutdown
//!
//! `ServerHandle::shutdown()` (or the host, on a signal) sets the shared
//! `watch` and cancels the shutdown token. The acceptor stops and the worker
//! loops stop reading their channels; every in-flight connection switches to
//! `graceful_shutdown`, then each worker drains its `JoinSet` with
//! `stream_drain_timeout` when an SSE response is active and `drain_timeout`
//! otherwise. Connections still running after the budget are aborted and the
//! worker threads are joined. The engine installs no OS signal handler itself:
//! the host owns signal wiring and calls [`ServerHandle::shutdown`].

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Semaphore};
use webx_core::error::{Error, Result};

use crate::shared::{ConnectionPermit, Shared};
use crate::worker::{self, Worker, WorkerFeed};

/// Pause before retrying a failed `accept` (EMFILE, transient network error).
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

/// Decrements `AdmissionStats.waiting` on every exit path, including future
/// cancellation while the acceptor waits for a slot.
struct AdmissionWaitGuard<'a>(&'a crate::shared::AdmissionStats);

impl Drop for AdmissionWaitGuard<'_> {
    fn drop(&mut self) {
        self.0.end_wait();
    }
}

/// One bound listener plus its transport configuration.
///
/// Multiple listeners deliberately share one worker pool and one global
/// connection governor. TLS is per listener because an http and an https URL
/// usually have different transport settings.
pub struct Listener {
    /// An already-bound blocking listener; the engine puts it in nonblocking
    /// mode on the runtime that owns the acceptor.
    pub listener: std::net::TcpListener,
    /// TLS configuration for this listener, or `None` for cleartext.
    pub tls: Option<Arc<rustls::ServerConfig>>,
}

impl Listener {
    /// Wrap a cleartext listener.
    pub fn plain(listener: std::net::TcpListener) -> Self {
        Self {
            listener,
            tls: None,
        }
    }

    /// Wrap a TLS listener.
    pub fn tls(listener: std::net::TcpListener, tls: Arc<rustls::ServerConfig>) -> Self {
        Self {
            listener,
            tls: Some(tls),
        }
    }
}

/// How long a worker thread may take to build its runtime before startup fails.
const WORKER_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Extra slack on top of the drain budget when joining worker threads.
const JOIN_GRACE: Duration = Duration::from_secs(10);

/// A fully assembled engine, ready to run on an already-bound listener.
pub struct Server {
    pub shared: Arc<Shared>,
}

impl Server {
    /// Wrap the assembled [`Shared`] state. The host builds it; the engine only
    /// consumes it.
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// A handle that can request graceful shutdown from application code.
    ///
    /// The signal is stored in a `watch`, so calling it before [`Self::run`]
    /// starts is not lost.
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            shutdown: self.shared.shutdown.clone(),
            token: self.shared.shutdown_token.clone(),
        }
    }

    /// Serve one already-connected byte stream through the engine pipeline.
    ///
    /// Unlike [`Self::run`]/[`Self::run_listeners`], the caller owns accept and
    /// TLS: this entry point takes any `AsyncRead + AsyncWrite` stream and runs
    /// the production hyper connection loop, router, middleware and drain logic
    /// on it. It backs the in-process wire test harness and custom transport
    /// adapters (for example a future HTTP/3 listener); it does not consume
    /// admission slots or set the single-run lifecycle flag, so a test may open
    /// several streams against one assembled server.
    pub async fn serve_stream<I>(&self, io: I, peer: SocketAddr)
    where
        I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        worker::serve_stream(Arc::clone(&self.shared), io, peer).await;
    }

    /// Serve one neutral request through the engine pipeline.
    ///
    /// Listener backends that multiplex requests themselves (HTTP/3 streams,
    /// in-process test servers) call this per request. Router, middleware,
    /// handler dispatch, binding, error writer, panic boundary, response
    /// assembly and SSE lifecycle are the production ones.
    pub async fn serve_transport_request(
        &self,
        request: crate::transport::TransportRequest,
    ) -> crate::transport::TransportResponse {
        let terminal = connection::final_handler(Arc::clone(&self.shared));
        connection::serve_transport(Arc::clone(&self.shared), terminal, request).await
    }

    /// Run a listener-level transport backend.
    ///
    /// The backend owns accept and transport-level handshakes (TLS, QUIC, ...);
    /// the engine hands it a [`TransportRuntime`](crate::transport::TransportRuntime)
    /// carrying the shared state, shutdown signal and the process-wide admission
    /// governor ([`TransportRuntime::admit_connection`](crate::transport::TransportRuntime::admit_connection)). The backend
    /// submits each decoded request through [`Self::serve_transport_request`].
    pub async fn run_transport(
        &self,
        transport: Arc<dyn crate::transport::IHttpTransport>,
    ) -> Result<()> {
        let metrics = self.shared.admission.register_transport(transport.kind());
        let runtime = crate::transport::TransportRuntime::new(
            Arc::clone(&self.shared),
            self.shared.subscribe_shutdown(),
        )
        .with_transport_metrics(metrics);
        transport.run(runtime).await
    }

    /// Serve one listener until shutdown.
    ///
    /// This is the single-listener convenience wrapper. `config.tls` is
    /// honoured here if `Shared.tls` was not pre-built, so the published
    /// `EngineConfig` cannot silently fall back to cleartext.
    pub async fn run(self, listener: std::net::TcpListener) -> Result<()> {
        let tls = match self.shared.tls.clone() {
            Some(config) => Some(crate::tls::with_key_log(config, self.shared.config.mode)),
            None => match self.shared.config.tls.as_ref() {
                Some(config) => Some(crate::tls::with_key_log(
                    crate::tls::build_server_config_with_client_auth(
                        &config.cert_path,
                        &config.key_path,
                        &config.client_ca_path,
                        config.client_auth,
                    )?,
                    self.shared.config.mode,
                )),
                None => None,
            },
        };
        self.run_listeners(vec![Listener { listener, tls }]).await
    }

    /// Serve all listeners with one worker pool and one global connection
    /// governor.
    ///
    /// This is the production entry point for `App.Urls`: http and https URLs
    /// share `App.MaxConnections` and the same worker threads, instead of
    /// multiplying both by the number of listeners.
    pub async fn run_listeners(self, listeners: Vec<Listener>) -> Result<()> {
        if listeners.is_empty() {
            return Err(Error::bad_request(
                "engine has no listener to serve".to_string(),
            ));
        }
        crate::config::validate_max_connections(self.shared.config.max_connections)?;

        let shared = Arc::clone(&self.shared);
        if shared.started.swap(true, Ordering::AcqRel) {
            return Err(Error::state(
                "engine Shared is single-use; build a new engine for another run".to_string(),
            ));
        }
        // If the caller's `run` future is dropped (timeout/select/abort), do not
        // leave detached worker threads draining for the SSE budget or serving
        // requests after the supervisor believes the server stopped.
        let mut run_guard = ServerRunGuard::new(Arc::clone(&shared));
        let workers = shared.config.resolved_workers();
        tracing::info!(
            workers,
            max_connections = shared.config.max_connections,
            listeners = listeners.len(),
            "engine accepting connections"
        );

        let (handles, startup_error) = self.spawn_workers(&shared, workers).await;
        if let Some(error) = startup_error {
            tracing::error!(error = %error, "engine worker startup failed");
            shared.trigger_shutdown();
            let join_failure = join_workers(handles, join_budget(&shared)).await;
            if join_failure.is_none() {
                run_guard.disarm();
            }
            return Err(error);
        }

        let feeds = Arc::new(
            handles
                .iter()
                .map(|worker| worker.feed.clone())
                .collect::<Vec<_>>(),
        );
        let governor = shared.admission.governor(shared.config.max_connections);
        let mut acceptors = tokio::task::JoinSet::new();
        for listener in listeners {
            let shared = Arc::clone(&shared);
            let feeds = Arc::clone(&feeds);
            let governor = Arc::clone(&governor);
            acceptors.spawn(async move { accept_loop(shared, listener, feeds, governor).await });
        }

        let mut accept_error: Option<Error> = None;
        while let Some(result) = acceptors.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(error = %error, "engine acceptor failed");
                    shared.trigger_shutdown();
                    if accept_error.is_none() {
                        accept_error = Some(error);
                    }
                }
                Err(error) => {
                    tracing::error!(error = %error, "engine acceptor task failed");
                    shared.trigger_shutdown();
                    if accept_error.is_none() {
                        accept_error = Some(Error::internal(format!(
                            "engine acceptor task failed: {error}"
                        )));
                    }
                }
            }
        }

        // Every acceptor has stopped (shutdown, error, or worker loss).
        shared.trigger_shutdown();
        let join_failure = join_workers(handles, join_budget(&shared)).await;
        if join_failure.is_none() {
            run_guard.disarm();
        }

        match (accept_error, join_failure) {
            (Some(error), _) => Err(error),
            (None, Some(message)) => Err(Error::internal(message)),
            (None, None) => Ok(()),
        }
    }

    /// Spawn all worker threads and wait until each reports a live runtime.
    ///
    /// A worker that cannot build its runtime (thread limit, OS resource
    /// exhaustion) is a typed startup failure, not a silent partial pool.
    async fn spawn_workers(
        &self,
        shared: &Arc<Shared>,
        workers: usize,
    ) -> (Vec<Worker>, Option<Error>) {
        let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
        let mut handles = Vec::with_capacity(workers);

        for index in 0..workers {
            match worker::spawn(index, Arc::clone(shared), ready_tx.clone()) {
                Ok(worker) => handles.push(worker),
                Err(error) => return (handles, Some(error)),
            }
        }
        // Drop our copy so a worker that dies before reporting closes the
        // channel instead of leaving startup to the timeout.
        drop(ready_tx);

        for _ in 0..handles.len() {
            match tokio::time::timeout(WORKER_STARTUP_TIMEOUT, ready_rx.recv()).await {
                Ok(Some(Ok(()))) => {}
                Ok(Some(Err(message))) => {
                    return (handles, Some(Error::internal(message)));
                }
                Ok(None) => {
                    return (
                        handles,
                        Some(Error::internal(
                            "an engine worker died before reporting ready".to_string(),
                        )),
                    );
                }
                Err(_) => {
                    return (
                        handles,
                        Some(Error::internal(format!(
                            "an engine worker did not start within {}s",
                            WORKER_STARTUP_TIMEOUT.as_secs()
                        ))),
                    );
                }
            }
        }

        (handles, None)
    }
}

/// Cancellation guard for one `Server::run_listeners` invocation.
struct ServerRunGuard {
    shared: Arc<Shared>,
    armed: bool,
}

impl ServerRunGuard {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ServerRunGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shared.trigger_immediate_shutdown();
        }
    }
}

/// Handle for requesting graceful shutdown of a running [`Server`].
#[derive(Clone)]
pub struct ServerHandle {
    pub shutdown: watch::Sender<bool>,
    pub token: webx_core::hosted::CancellationToken,
}

impl ServerHandle {
    /// Stop accepting, cancel long-lived responses, drain and join workers.
    ///
    /// Safe to call before [`Server::run`] (the `watch` keeps the value) and
    /// more than once.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
        self.token.cancel();
    }
}

/// Accept connections until shutdown, handing each to the shared worker pool.
async fn accept_loop(
    shared: Arc<Shared>,
    listener: Listener,
    workers: Arc<Vec<WorkerFeed>>,
    governor: Arc<Semaphore>,
) -> Result<()> {
    let Listener {
        listener: raw_listener,
        tls: explicit_tls,
    } = listener;
    raw_listener.set_nonblocking(true).map_err(|err| {
        Error::bad_request(format!(
            "engine listener could not be put in nonblocking mode: {err}"
        ))
    })?;
    let listener = TcpListener::from_std(raw_listener).map_err(|err| {
        Error::bad_request(format!("engine listener could not join the runtime: {err}"))
    })?;
    // `Server::run` resolves the default listener's TLS before calling here;
    // `run_listeners` is explicit so one listener's config never leaks onto
    // another (for example an https URL's certificate onto an http URL).
    let tls = explicit_tls;

    let mut shutdown = shared.subscribe_shutdown();
    let mut cursor = 0usize;

    loop {
        if *shutdown.borrow() {
            break;
        }

        let accepted = tokio::select! {
            biased;
            _ = worker::wait_for_shutdown(&mut shutdown) => break,
            accepted = listener.accept() => accepted,
        };

        let (stream, peer) = match accepted {
            Ok(connection) => connection,
            Err(err) => {
                tracing::warn!(error = %err, "engine accept failed; retrying");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                continue;
            }
        };

        // Admission is global: no per-listener or per-worker multiplication.
        // A socket that cannot get a slot within `AdmissionTimeoutMs` answers
        // `503` and is closed: leaving it in the kernel backlog would hide the
        // queue from clients and from `/metrics`.
        shared.admission.begin_wait();
        // RAII: the acceptor future can be dropped while waiting for a slot
        // (shutdown, task cancellation); an explicit `end_wait()` on the two
        // select branches would leak a `waiting` counter in that case.
        let wait_guard = AdmissionWaitGuard(&shared.admission);
        let permit = tokio::select! {
            biased;
            _ = worker::wait_for_shutdown(&mut shutdown) => {
                break;
            }
            acquired = acquire_admission(&governor, shared.config.admission_timeout) => {
                match acquired {
                    Admission::Permit(raw) => {
                        ConnectionPermit::new(raw, Arc::clone(&shared.admission))
                    }
                    Admission::TimedOut => {
                        shared.admission.record_rejected();
                        reject_overloaded(stream, peer).await;
                        continue;
                    }
                    Admission::Closed => {
                        return Err(Error::internal("connection governor closed".to_string()));
                    }
                }
            }
        };
        drop(wait_guard);
        shared.admission.record_admitted();

        if let Err(err) = stream.set_nodelay(true) {
            tracing::debug!(peer = %peer, error = %err, "engine: TCP_NODELAY unavailable");
        }

        // Deregisters the socket from the acceptor runtime without `unsafe`;
        // the worker registers it on its own runtime with `from_std`.
        let stream = match stream.into_std() {
            Ok(stream) => stream,
            Err(err) => {
                tracing::debug!(peer = %peer, error = %err, "engine: connection handoff failed");
                continue;
            }
        };

        handoff(
            &workers,
            &mut cursor,
            stream,
            peer,
            tls.clone(),
            permit,
            &mut shutdown,
        )
        .await?;
    }

    tracing::debug!("engine acceptor stopped");
    Ok(())
}

/// One admission attempt result.
enum Admission {
    /// A slot was granted.
    Permit(tokio::sync::OwnedSemaphorePermit),
    /// The socket waited past `AdmissionTimeout`.
    TimedOut,
    /// The governor was closed (engine shutting down).
    Closed,
}

/// Acquire a global admission slot, bounded by the configured deadline.
async fn acquire_admission(governor: &Arc<Semaphore>, timeout: Option<Duration>) -> Admission {
    // Clone first so the future owns the permit source; `acquire_owned` is
    // cancellation-safe (a dropped future releases nothing because no permit
    // was granted).
    let acquire = Arc::clone(governor).acquire_owned();
    match timeout {
        Some(limit) => match tokio::time::timeout(limit, acquire).await {
            Ok(Ok(permit)) => Admission::Permit(permit),
            Ok(Err(_)) => Admission::Closed,
            Err(_) => Admission::TimedOut,
        },
        None => match acquire.await {
            Ok(permit) => Admission::Permit(permit),
            Err(_) => Admission::Closed,
        },
    }
}

/// Best-effort `503` for a socket that waited past the admission deadline.
///
/// The write is bounded so a client that never reads cannot pin the acceptor;
/// HTTP/2 clients get the same bytes and then a close (the connection is being
/// shed anyway).
const OVERLOAD_RESPONSE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\
content-type: text/plain; charset=utf-8\r\n\
retry-after: 1\r\n\
content-length: 0\r\n\
connection: close\r\n\r\n";

async fn reject_overloaded(mut stream: tokio::net::TcpStream, peer: SocketAddr) {
    use tokio::io::AsyncWriteExt;
    tracing::debug!(peer = %peer, "engine: admission timed out; answering 503");
    let _ = tokio::time::timeout(
        Duration::from_millis(50),
        stream.write_all(OVERLOAD_RESPONSE),
    )
    .await;
    let _ = stream.shutdown().await;
}

/// Insert `stream` into the least-loaded worker that still has budget.
///
/// Selection is least-loaded (queue + active connections) with a round-robin
/// tie-break, then the worker's connection-budget permit gates admission. When
/// every worker is saturated the acceptor waits for capacity on the chosen
/// worker **while still observing shutdown**, so a saturated server can always
/// be stopped.
#[allow(clippy::needless_return)]
async fn handoff(
    workers: &[WorkerFeed],
    cursor: &mut usize,
    stream: std::net::TcpStream,
    peer: SocketAddr,
    tls: Option<Arc<rustls::ServerConfig>>,
    permit: crate::shared::ConnectionPermit,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    if workers.is_empty() {
        return Err(Error::internal(
            "engine has no worker to accept connections".to_string(),
        ));
    }

    let count = workers.len();
    let start = *cursor % count;
    let mut chosen = start;
    let mut chosen_load = usize::MAX;
    for offset in 0..count {
        let index = (start + offset) % count;
        let load = worker_load(&workers[index]);
        if load < chosen_load {
            chosen = index;
            chosen_load = load;
            if load == 0 {
                break;
            }
        }
    }
    *cursor = (chosen + 1) % count;
    let worker = &workers[chosen];

    // The gauge is owned by the message from this point on. If the message is
    // dropped by a full-channel shutdown or a closed worker, `Drop` releases it;
    // the worker drops it when it dequeues. No manual decrement races.
    let pending = worker::PendingGuard::new(Arc::clone(&worker.pending));
    let handoff = worker::Handoff {
        stream,
        peer,
        tls,
        permit,
        pending,
    };

    match worker.tx.try_send(handoff) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(handoff)) => {
            // The channel is bounded; wait for room instead of dropping a
            // client that already connected. On shutdown the send future is
            // dropped together with the handoff and its permit/gauge.
            tokio::select! {
                biased;
                _ = worker::wait_for_shutdown(shutdown) => Ok(()),
                sent = worker.tx.send(handoff) => match sent {
                    Ok(()) => Ok(()),
                    Err(_) => Err(Error::internal("engine worker channel closed".to_string())),
                },
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(Error::internal("engine worker channel closed".to_string()))
        }
    }
}

fn worker_load(worker: &WorkerFeed) -> usize {
    worker.pending.load(Ordering::Relaxed) + worker.active.load(Ordering::Relaxed)
}

fn join_budget(shared: &Shared) -> Duration {
    join_budget_for(
        shared.config.drain_timeout,
        shared.config.stream_drain_timeout,
    )
}

fn join_budget_for(drain_timeout: Duration, stream_drain_timeout: Duration) -> Duration {
    drain_timeout.max(stream_drain_timeout) + JOIN_GRACE
}

/// Join every worker thread off the async runtime.
///
/// Returns the first failure message; `None` means every worker stopped
/// cleanly. The join runs on the blocking pool because `std::thread::JoinHandle`
/// has no async counterpart, and it is bounded so a stuck worker cannot hang
/// shutdown forever.
async fn join_workers(workers: Vec<Worker>, budget: Duration) -> Option<String> {
    let join_all = async move {
        let mut failure: Option<String> = None;
        for worker in workers {
            match tokio::task::spawn_blocking(move || worker.join()).await {
                Ok(Ok(())) => {}
                Ok(Err(message)) => {
                    if failure.is_none() {
                        failure = Some(message);
                    }
                }
                Err(err) => {
                    if failure.is_none() {
                        failure = Some(format!("engine worker join task failed: {err}"));
                    }
                }
            }
        }
        failure
    };

    match tokio::time::timeout(budget, join_all).await {
        Ok(failure) => failure,
        Err(_) => Some(format!(
            "engine workers did not stop within {}s of shutdown",
            budget.as_secs()
        )),
    }
}

// ---------------------------------------------------------------------------
// Request handling
// ---------------------------------------------------------------------------

pub(crate) mod connection {
    //! One request: context -> pipeline -> router terminal -> response.
    //!
    //! The final handler is the pipeline's terminal stage. It matches the route,
    //! records the matched pattern and params on the context, and dispatches the
    //! target; an unmatched request is answered by the router's 404/405 path.

    use std::convert::Infallible;
    use std::future::Future;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::Arc;

    use futures_util::FutureExt;
    use hyper::body::Incoming;
    use hyper::{Request, Response};
    use webx_core::error::{Error, Result};
    use webx_core::http::IHttpContext;
    use webx_core::problem::{retry_after_of, ProblemDetails, X_WEBX_TITLE};

    use crate::context::EngineContext;
    use crate::pipeline::FinalHandler;
    use crate::response::RespBody;
    use crate::router::RouteTarget;
    use crate::shared::{ActiveStreamGuard, Shared};

    /// Serve one request on an existing connection.
    ///
    /// Errors from middleware/handlers are converted into a response here, so
    /// hyper always receives `Ok` and the connection can be reused. A panic in
    /// user code is contained at this boundary and becomes a 500 instead of a
    /// dropped connection (or a dead worker under `panic = "abort"`).
    pub(crate) async fn serve(
        shared: Arc<Shared>,
        request: Request<Incoming>,
        peer: Option<SocketAddr>,
        streams: Arc<ActiveStreamGuard>,
        client_cert: Option<Arc<str>>,
        terminal: FinalHandler,
        upgrade: Option<hyper::upgrade::OnUpgrade>,
    ) -> std::result::Result<Response<RespBody>, Infallible> {
        // The TLS terminator is the only source of truth for the client
        // certificate: it is carried as the neutral "verified fingerprint"
        // field, and `EngineContext::from_transport` performs the strip-then-write
        // on the header for every transport (hyper included).
        let (parts, body) = request.into_parts();
        let neutral = match client_cert {
            Some(fingerprint) => crate::hyper_compat::request(parts, body, peer)
                .with_client_cert_fingerprint(fingerprint),
            None => crate::hyper_compat::request(parts, body, peer),
        };
        let mut ctx = EngineContext::from_transport(
            neutral,
            Arc::clone(&shared.body_limits),
            Some(Arc::clone(&shared.dispatch)),
        );
        // Only upgrade-bearing requests need the drain tracker: a regular
        // request must not pay an extra shared-state clone on the hot path.
        if upgrade.is_some() {
            ctx.track_upgrade_sessions(Arc::clone(&shared));
        }
        if let Some(upgrade) = upgrade {
            ctx = ctx.with_upgrade(upgrade);
        }

        execute_pipeline(Arc::clone(&shared), terminal, &mut ctx).await;

        let response = ctx.finalize().await;
        if is_sse_response(&response) {
            // Mark once per connection; the guard releases the slot when the
            // connection ends (including on abort), not when the response ends.
            streams.mark();
        }
        Ok(response)
    }

    /// Run the middleware pipeline and panic boundary for one request context.
    ///
    /// Shared by the hyper service wrapper and the neutral transport port so
    /// router/error/panic behavior has exactly one implementation.
    async fn execute_pipeline(
        shared: Arc<Shared>,
        terminal: FinalHandler,
        ctx: &mut EngineContext,
    ) {
        let error_writer: crate::pipeline::ErrorHandler = {
            let shared = Arc::clone(&shared);
            Arc::new(move |ctx: &mut EngineContext, error: Error| {
                let shared = Arc::clone(&shared);
                Box::pin(async move {
                    write_error(&shared, ctx, &error).await;
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>
            })
        };

        let outcome = std::panic::AssertUnwindSafe(shared.pipeline.execute_with_error(
            ctx,
            terminal,
            error_writer,
        ))
        .catch_unwind()
        .await;

        if let Err(payload) = outcome {
            tracing::error!(panic = %panic_message(&payload), "engine: request panicked");
            // The error writer itself may have panicked. Use a fixed
            // problem+json response, outside the failing writer path.
            write_problem(ctx, problem_details(500, "an unexpected error occurred")).await;
        }
    }

    /// Serve one neutral transport request through the same pipeline.
    ///
    /// The existing finalize pipeline (security headers, HEAD/204/304 handling,
    /// file/range assembly and SSE lifecycle observation) produces the neutral
    /// response, so a non-hyper transport gets the same bytes and lifecycle
    /// events as the hyper path. The client-certificate strip/inject contract is
    /// part of [`EngineContext::from_transport`], so every transport entry point
    /// (including a custom backend calling that constructor directly) inherits
    /// it.
    pub(crate) async fn serve_transport(
        shared: Arc<Shared>,
        terminal: FinalHandler,
        request: crate::transport::TransportRequest,
    ) -> crate::transport::TransportResponse {
        let has_upgrade = request.upgrade.is_some();
        let mut ctx = EngineContext::from_transport(
            request,
            Arc::clone(&shared.body_limits),
            Some(Arc::clone(&shared.dispatch)),
        );
        if has_upgrade {
            ctx.track_upgrade_sessions(Arc::clone(&shared));
        }

        execute_pipeline(Arc::clone(&shared), terminal, &mut ctx).await;
        ctx.finalize_transport().await
    }

    /// Build the pipeline terminal for one worker.
    ///
    /// The returned handler captures the [`Shared`] state; the worker builds it
    /// once and clones the `Arc` per request.
    pub(crate) fn final_handler(shared: Arc<Shared>) -> FinalHandler {
        Arc::new(move |ctx: &mut EngineContext| route(Arc::clone(&shared), ctx))
    }

    /// Run a matched route target, preferring compile-time direct dispatch
    /// (2.4-9) so the common JSON route skips the `async_trait` boxed future.
    async fn dispatch_target(target: &dyn RouteTarget, ctx: &mut EngineContext) -> Result<()> {
        if let Some((dispatch, encoder)) = target.direct() {
            // The direct path skips the route guard, so it must keep the one
            // guard that is not optional: the declared body limit check.
            ctx.request_mut().validate_declared_body_limit()?;
            let result = dispatch(&mut *ctx, encoder).await?;
            ctx.write_endpoint_result(result).await
        } else {
            target.handle(ctx).await
        }
    }

    /// Route one request and dispatch its target.
    fn route<'a>(
        shared: Arc<Shared>,
        ctx: &'a mut EngineContext,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // A rejection recorded while the context was built (e.g. an
            // oversized declared `Content-Length`) must surface from inside the
            // pipeline so CORS/security headers and metrics still run and the
            // `after` hooks observe the error.
            if let Some(error) = ctx.take_early_error() {
                return Err(error);
            }

            // Own the match result before touching `ctx` mutably again.
            let matched = {
                let request = ctx.request();
                shared.router.match_route(request.method(), request.path())
            };

            match matched {
                Some(hit) => {
                    ctx.set_route(hit.pattern, hit.params);
                    // The route target applies a possible `#[max_body_size]`
                    // and validates the declared length itself, so a route may
                    // raise the global default. Unmatched requests have no
                    // route limit and validate against the global cap below.
                    match shared.config.request_timeout {
                        Some(limit) => {
                            match tokio::time::timeout(limit, dispatch_target(&*hit.target, ctx))
                                .await
                            {
                                Ok(result) => result,
                                Err(_) => Err(Error::status(504, "request timed out".to_string())),
                            }
                        }
                        None => dispatch_target(&*hit.target, ctx).await,
                    }
                }
                None => {
                    // Preserve the authoritative global check for 404/405: an
                    // oversized declared body is rejected before routing only
                    // when no route owns a per-route limit.
                    ctx.request_mut().validate_declared_body_limit()?;
                    handle_no_route(&shared, ctx).await
                }
            }
        })
    }

    /// Answer a request no route accepted.
    ///
    /// `Router::match_route` returns `None` for exactly two cases: the path
    /// exists under another method (-> `Allow` + 204 for OPTIONS / 405), and a
    /// genuine miss with no fallback configured (-> 404). A configured fallback
    /// is returned by `match_route` itself, so the SPA/404 decision stays in the
    /// fallback target and this function never needs a fallback accessor.
    async fn handle_no_route(shared: &Shared, ctx: &mut EngineContext) -> Result<()> {
        if ctx.response().has_body() {
            return Ok(());
        }

        let (path, path_exists, is_options) = {
            let request = ctx.request();
            (
                request.path().to_string(),
                shared.router.path_exists(request.path()),
                request.method().eq_ignore_ascii_case("OPTIONS"),
            )
        };

        if path_exists {
            let allow = shared.router.allowed_methods(&path).join(", ");
            ctx.response_mut().set_header("allow", &allow);
            if is_options {
                // RFC 9110 9.3.7: OPTIONS reports the methods the path allows.
                ctx.response_mut().set_status(204);
            } else {
                // RFC 9110 15.5.6: a 405 must carry `Allow` (set above).
                write_problem(ctx, problem_details(405, "Method Not Allowed")).await;
            }
            return Ok(());
        }

        let accept = {
            ctx.request()
                .header("accept")
                .map(std::string::ToString::to_string)
        };
        let writer = shared
            .error_writers
            .iter()
            .find(|writer| writer.matches(None, &path, accept.as_deref()))
            .cloned();
        match writer {
            Some(writer) => {
                writer
                    .write(ctx, &Error::route_not_found("Not Found".to_string()))
                    .await
            }
            None => write_problem(ctx, problem_details(404, "Not Found")).await,
        }
        Ok(())
    }

    /// Write an error response, preferring the host's registered writers.
    async fn write_error(shared: &Shared, ctx: &mut EngineContext, error: &Error) {
        // The successful representation's validators/encoding/cache policy are
        // not valid for a problem response. Keep Allow/CORS/Vary.
        ctx.strip_entity_headers();
        let route = ctx.request().route_pattern().map(str::to_string);
        let path = ctx.request().path().to_string();
        let accept = ctx
            .request()
            .header("accept")
            .map(std::string::ToString::to_string);

        let writer = shared
            .error_writers
            .iter()
            .find(|writer| writer.matches(route.as_deref(), &path, accept.as_deref()))
            .cloned();

        match writer {
            Some(writer) => writer.write(ctx, error).await,
            None => write_default_problem(ctx, error).await,
        }
    }

    /// Minimal RFC 7807 writer used when the host registered no writer.
    ///
    /// 5xx details are logged and replaced with a stable message; 401 carries
    /// the `Bearer` challenge every framework 401 must have.
    /// Minimal RFC 7807 writer used when the host registered no writer.
    ///
    /// 5xx details are logged and replaced with a stable message; 401 carries
    /// the `Bearer` challenge every framework 401 must have. A problem that was
    /// pre-built by the application (verbatim `x-webx-title`) keeps its exact
    /// shape: no sanitization, no 401/429 headers, no 5xx log.
    async fn write_default_problem(ctx: &mut EngineContext, error: &Error) {
        let path = ctx.request().path().to_string();
        let trace_id = ctx.trace_id().map(str::to_string);
        let status = error.status_code();
        let verbatim = error.extensions().contains_key(X_WEBX_TITLE);

        if !verbatim {
            if status == 401 {
                ctx.response_mut()
                    .set_header("www-authenticate", "Bearer realm=\"rust-webx\"");
            }
            // A typed rate-limit error carries the RFC 6585 wait value; this is
            // the only writer that turns a handler `Error` into a response, so
            // the header is written exactly once.
            if status == 429 {
                if let Some(retry_after_secs) = retry_after_of(error) {
                    ctx.response_mut()
                        .set_header("retry-after", &retry_after_secs.to_string());
                }
            }
            if status >= 500 {
                tracing::error!(status, error = %error, "engine: unhandled server error");
            }
        }

        let problem = ProblemDetails::from_app_error(error, &path, trace_id.as_deref());
        write_problem(ctx, problem).await;
    }
    async fn write_problem(ctx: &mut EngineContext, mut problem: ProblemDetails) {
        // Problems built by internal helpers (404/405/deadline) have no `Error`
        // to derive from; fill the guaranteed extension members here.
        let path = ctx.request().path().to_string();
        let trace_id = ctx.trace_id().map(str::to_string);
        problem.complete(Some(&path), trace_id.as_deref());

        ctx.strip_entity_headers();
        let status = problem.status;
        // Serializing a `ProblemDetails` cannot fail; an empty body is still a
        // well-formed error response with the right status.
        let body = serde_json::to_vec(&problem).unwrap_or_default();

        ctx.response_mut().set_status(status);
        ctx.response_mut()
            .set_header("content-type", "application/problem+json");
        if let Err(error) = ctx.response_mut().write_bytes(body).await {
            tracing::debug!(error = %error, "engine: failed to write error response");
        }
    }

    fn problem_details(status: u16, detail: impl Into<String>) -> ProblemDetails {
        ProblemDetails {
            typ: Some(format!("https://httpstatuses.com/{status}")),
            title: status_title(status).to_string(),
            status,
            detail: Some(detail.into()),
            instance: None,
            extensions: None,
        }
    }

    fn status_title(status: u16) -> &'static str {
        match status {
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Error",
        }
    }

    /// Best-effort text for a caught panic payload.
    fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
        if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic payload".to_string()
        }
    }

    /// Whether a `content-type` marks an SSE response.
    pub(crate) fn is_sse_content_type(content_type: Option<&str>) -> bool {
        content_type.is_some_and(|value| value.trim_start().starts_with("text/event-stream"))
    }

    fn is_sse_response(response: &Response<RespBody>) -> bool {
        is_sse_content_type(
            response
                .headers()
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
        )
    }

    #[cfg(test)]
    mod tests {
        use super::is_sse_content_type;

        #[test]
        fn sse_detection_accepts_parameters() {
            assert!(is_sse_content_type(Some("text/event-stream")));
            assert!(is_sse_content_type(Some(
                "text/event-stream; charset=utf-8"
            )));
            assert!(is_sse_content_type(Some("text/event-stream;charset=utf-8")));
        }

        #[test]
        fn sse_detection_rejects_other_content_types() {
            assert!(!is_sse_content_type(Some("application/json")));
            assert!(!is_sse_content_type(Some("text/plain")));
            assert!(!is_sse_content_type(None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{acquire_admission, join_budget_for, Admission, JOIN_GRACE};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;

    #[test]
    fn join_budget_covers_the_longest_drain_plus_grace() {
        assert_eq!(
            join_budget_for(Duration::from_secs(1), Duration::from_secs(300)),
            Duration::from_secs(300) + JOIN_GRACE
        );
        assert_eq!(
            join_budget_for(Duration::from_secs(30), Duration::from_secs(5)),
            Duration::from_secs(30) + JOIN_GRACE
        );
    }

    #[tokio::test]
    async fn admission_timeout_reports_timed_out() {
        let governor = Arc::new(Semaphore::new(0));
        let started = std::time::Instant::now();
        let outcome = acquire_admission(&governor, Some(Duration::from_millis(20))).await;
        assert!(matches!(outcome, Admission::TimedOut));
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[tokio::test]
    async fn admission_without_deadline_waits_for_a_permit() {
        let governor = Arc::new(Semaphore::new(0));
        let waiter = {
            let governor = Arc::clone(&governor);
            tokio::spawn(async move { acquire_admission(&governor, None).await })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        governor.add_permits(1);
        let outcome = waiter.await.expect("join admission waiter");
        assert!(matches!(outcome, Admission::Permit(_)));
    }
}
