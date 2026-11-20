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
use tokio::sync::{mpsc, watch};
use webx_core::error::{Error, Result};

use crate::shared::Shared;
use crate::worker::{self, Worker};

/// Pause before retrying a failed `accept` (EMFILE, transient network error).
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

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

    /// Serve on `listener` until shutdown, then drain and join all workers.
    ///
    /// The listener is converted onto the caller's runtime; connections are
    /// accepted there and handed to worker threads. Binding still belongs to
    /// the caller so a port conflict fails startup before any worker is
    /// spawned.
    pub async fn run(self, listener: std::net::TcpListener) -> Result<()> {
        let shared = Arc::clone(&self.shared);

        listener.set_nonblocking(true).map_err(|err| {
            Error::Http(format!(
                "engine listener could not be put in nonblocking mode: {err}"
            ))
        })?;
        let listener = TcpListener::from_std(listener).map_err(|err| {
            Error::Http(format!("engine listener could not join the runtime: {err}"))
        })?;

        let workers = shared.config.resolved_workers();
        let budget = worker::connection_budget(shared.config.max_connections, workers);
        tracing::info!(
            workers,
            connection_budget_per_worker = budget,
            max_connections = shared.config.max_connections,
            tls = shared.tls.is_some(),
            "engine accepting connections"
        );

        let (handles, startup_error) = self.spawn_workers(&shared, workers).await;
        if let Some(error) = startup_error {
            tracing::error!(error = %error, "engine worker startup failed");
            shared.trigger_shutdown();
            let _ = join_workers(handles, join_budget(&shared)).await;
            return Err(error);
        }

        let accept_result = accept_loop(&shared, &listener, &handles).await;

        // The acceptor also exits when a worker disappears; stopping the whole
        // server is the only safe answer to a dead worker.
        shared.trigger_shutdown();
        drop(listener);

        let join_failure = join_workers(handles, join_budget(&shared)).await;

        match (accept_result, join_failure) {
            (Err(error), _) => Err(error),
            (Ok(()), Some(message)) => Err(Error::Internal(message)),
            (Ok(()), None) => Ok(()),
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
                    return (handles, Some(Error::Internal(message)));
                }
                Ok(None) => break,
                Err(_) => {
                    return (
                        handles,
                        Some(Error::Internal(format!(
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

/// Accept connections until shutdown, handing each to a worker.
async fn accept_loop(shared: &Shared, listener: &TcpListener, workers: &[Worker]) -> Result<()> {
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

        handoff(workers, &mut cursor, stream, peer, &mut shutdown).await?;
    }

    tracing::debug!("engine acceptor stopped");
    Ok(())
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
    workers: &[Worker],
    cursor: &mut usize,
    stream: std::net::TcpStream,
    peer: SocketAddr,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    if workers.is_empty() {
        return Err(Error::Internal(
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

    let permit = match Arc::clone(&worker.budget).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            tokio::select! {
                biased;
                _ = worker::wait_for_shutdown(shutdown) => return Ok(()),
                acquired = Arc::clone(&worker.budget).acquire_owned() => match acquired {
                    Ok(permit) => permit,
                    Err(_) => {
                        return Err(Error::Internal(
                            "engine connection gate is closed".to_string(),
                        ))
                    }
                },
            }
        }
    };

    worker::increment(&worker.pending);
    match worker.tx.try_send(worker::Handoff {
        stream,
        peer,
        permit,
    }) {
        Ok(()) => return Ok(()),
        Err(mpsc::error::TrySendError::Full(handoff)) => {
            worker::decrement(&worker.pending);
            // The channel is bounded; wait for room instead of dropping a
            // client that already connected.
            tokio::select! {
                biased;
                _ = worker::wait_for_shutdown(shutdown) => return Ok(()),
                sent = worker.tx.send(handoff) => match sent {
                    Ok(()) => {
                        worker::increment(&worker.pending);
                        return Ok(());
                    }
                    Err(_) => {
                        return Err(Error::Internal(
                            "engine worker channel closed".to_string(),
                        ))
                    }
                },
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            worker::decrement(&worker.pending);
            return Err(Error::Internal("engine worker channel closed".to_string()));
        }
    }
}

fn worker_load(worker: &Worker) -> usize {
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
    use webx_core::problem::ProblemDetails;

    use crate::context::EngineContext;
    use crate::pipeline::FinalHandler;
    use crate::response::RespBody;
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
        terminal: FinalHandler,
    ) -> std::result::Result<Response<RespBody>, Infallible> {
        let mut ctx = EngineContext::new(
            request,
            Arc::clone(&shared.body_limits),
            peer,
            Arc::clone(&shared.dispatch),
        );

        let outcome = std::panic::AssertUnwindSafe(shared.pipeline.execute(&mut ctx, terminal))
            .catch_unwind()
            .await;

        let result = match outcome {
            Ok(result) => result,
            Err(payload) => {
                tracing::error!(panic = %panic_message(&payload), "engine: request panicked");
                Err(Error::Internal("request handler panicked".to_string()))
            }
        };

        if let Err(error) = result {
            write_error(&shared, &mut ctx, &error).await;
        }

        let response = ctx.finalize().await;
        if is_sse_response(&response) {
            // Mark once per connection; the guard releases the slot when the
            // connection ends (including on abort), not when the response ends.
            streams.mark();
        }
        Ok(response)
    }

    /// Build the pipeline terminal for one worker.
    ///
    /// The returned handler captures the [`Shared`] state; the worker builds it
    /// once and clones the `Arc` per request.
    pub(crate) fn final_handler(shared: Arc<Shared>) -> FinalHandler {
        Arc::new(move |ctx: &mut EngineContext| route(Arc::clone(&shared), ctx))
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
                    match shared.config.request_timeout {
                        Some(limit) => {
                            match tokio::time::timeout(limit, hit.target.handle(ctx)).await {
                                Ok(result) => result,
                                Err(_) => Err(Error::Status(504, "request timed out".to_string())),
                            }
                        }
                        None => hit.target.handle(ctx).await,
                    }
                }
                None => handle_no_route(&shared, ctx).await,
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
                    .write(ctx, &Error::Routing("Not Found".to_string()))
                    .await
            }
            None => write_problem(ctx, problem_details(404, "Not Found")).await,
        }
        Ok(())
    }

    /// Write an error response, preferring the host's registered writers.
    async fn write_error(shared: &Shared, ctx: &mut EngineContext, error: &Error) {
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
    async fn write_default_problem(ctx: &mut EngineContext, error: &Error) {
        if let Error::Problem(problem) = error {
            // A binding/validation failure already carries its full payload
            // (status and the `errors` extension); write it verbatim.
            write_problem(ctx, (**problem).clone()).await;
            return;
        }

        let status = error.status_code();
        if status == 401 {
            ctx.response_mut()
                .set_header("www-authenticate", "Bearer realm=\"rust-webx\"");
        }

        let detail = if status >= 500 {
            tracing::error!(status, error = %error, "engine: unhandled server error");
            "an unexpected error occurred".to_string()
        } else {
            error
                .public_detail()
                .unwrap_or_else(|| status_title(status))
                .to_string()
        };

        write_problem(ctx, problem_details(status, detail)).await;
    }

    async fn write_problem(ctx: &mut EngineContext, problem: ProblemDetails) {
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
    use super::{join_budget_for, JOIN_GRACE};
    use std::time::Duration;

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
}
