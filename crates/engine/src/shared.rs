//! The handle every worker shares.
//!
//! One [`Shared`] is built by the host adapter and cloned (cheaply — every field
//! is an `Arc` or a cloneable channel handle) into every worker thread and
//! connection task. It deliberately holds **no** per-connection state: the
//! worker owns its runtime, connection and socket, the [`EngineContext`] owns
//! the request/response, and `Shared` only carries the immutable configuration
//! plus the process-wide shutdown signal and stream counter.
//!
//! [`EngineContext`]: crate::EngineContext

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::watch;
use webx_core::dispatch_runtime::DispatchRuntime;
use webx_core::error::IErrorResponseWriter;
use webx_core::hosted::CancellationToken;

use crate::body::BodyLimits;
use crate::config::EngineConfig;
use crate::pipeline::Pipeline;
use crate::router::Router;

/// Immutable engine state plus the shutdown signal, shared by all workers.
///
/// `Clone` only bumps `Arc`s and clones channel handles, so it is safe to clone
/// per worker/per connection. All fields are `Send + Sync`, which is what lets
/// the acceptor (caller runtime) hand [`Shared`] to worker threads.
#[derive(Clone)]
pub struct Shared {
    /// Worker count, limits, drain budgets and TLS material.
    pub config: Arc<EngineConfig>,
    /// Compiled route table (matchit per method + fallback).
    pub router: Arc<Router>,
    /// Built-in stages plus user `IMiddleware` adapters.
    pub pipeline: Arc<Pipeline>,
    /// DI handles of the host that dispatched this request.
    pub dispatch: Arc<DispatchRuntime>,
    /// Body/upload limits used by the request context.
    pub body_limits: Arc<BodyLimits>,
    /// Host error writers, tried in order before the default `problem+json`.
    pub error_writers: Arc<Vec<Arc<dyn IErrorResponseWriter>>>,
    /// TLS server configuration; `None` serves cleartext HTTP.
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// `true` once graceful shutdown started.
    ///
    /// A `watch` (not `Notify`) because the signal must not be lost: a worker
    /// or connection that subscribes after shutdown was requested still sees
    /// it. `send_replace` stores the value even without receivers, so
    /// `handle.shutdown()` before `run()` works.
    pub shutdown: watch::Sender<bool>,
    /// Cancelled together with [`Shared::shutdown`] so long-lived responses
    /// (SSE) can end with a `server_shutdown` event instead of being aborted at
    /// the drain deadline.
    pub shutdown_token: CancellationToken,
    /// Number of connections currently serving at least one SSE response.
    ///
    /// Drives the drain budget: while this is non-zero the server uses
    /// `stream_drain_timeout` instead of `drain_timeout`.
    pub active_streams: Arc<AtomicUsize>,
}

impl Shared {
    /// Subscribe to the shutdown signal (immediately `true` when already set).
    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// Whether graceful shutdown already started.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Raise the shutdown signal: stop accepting, cancel long-lived streams.
    ///
    /// Idempotent; every later call only re-stores `true` and re-cancels an
    /// already-cancelled token.
    pub fn trigger_shutdown(&self) {
        self.shutdown.send_replace(true);
        self.shutdown_token.cancel();
    }

    /// Guard that counts this connection as serving a stream while it lives.
    ///
    /// Call [`ActiveStreamGuard::mark`] when an SSE response is produced; the
    /// counter is decremented when the guard drops (connection end), so an
    /// aborted connection never leaks a "streaming" slot.
    pub fn active_stream_guard(&self) -> ActiveStreamGuard {
        ActiveStreamGuard::new(Arc::clone(&self.active_streams))
    }
}

/// `Drop` guard for [`Shared::active_streams`].
///
/// One guard exists per connection. The first SSE response on that connection
/// calls [`Self::mark`] — HTTP/2 multiplexes several requests over one
/// connection but the drain budget is per connection, so only the first counts.
/// The count is released when the guard — and therefore the connection task —
/// drops, including on abort.
#[derive(Debug)]
pub struct ActiveStreamGuard {
    counter: Arc<AtomicUsize>,
    marked: AtomicBool,
}

impl ActiveStreamGuard {
    /// Create a guard for `counter` that is not yet counted.
    pub fn new(counter: Arc<AtomicUsize>) -> Self {
        Self {
            counter,
            marked: AtomicBool::new(false),
        }
    }

    /// Record that this connection now serves a stream.
    ///
    /// Returns `true` on the transition (the counter was incremented), `false`
    /// when the connection was already marked.
    pub fn mark(&self) -> bool {
        if self.marked.swap(true, Ordering::Relaxed) {
            return false;
        }
        self.counter.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Whether this connection is (still) counted as streaming.
    pub fn is_marked(&self) -> bool {
        self.marked.load(Ordering::Relaxed)
    }
}

impl Drop for ActiveStreamGuard {
    fn drop(&mut self) {
        if self.marked.load(Ordering::Relaxed) {
            // Saturating compare-exchange keeps a double drop from wrapping the
            // counter and avoids the newly-deprecated `fetch_update` on newer
            // toolchains while staying compatible with the 1.95 MSRV.
            let mut current = self.counter.load(Ordering::Relaxed);
            while current > 0 {
                match self.counter.compare_exchange_weak(
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_is_send_and_sync() {
        // The acceptor runs on the caller runtime and hands `Shared` to worker
        // threads; a non-Send field would only fail at the spawn site.
        assert_send_sync::<Shared>();
        assert_send_sync::<ActiveStreamGuard>();
    }

    #[test]
    fn stream_guard_counts_once_and_releases_on_drop() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let guard = ActiveStreamGuard::new(Arc::clone(&counter));
            assert!(guard.mark(), "first mark increments");
            assert!(!guard.mark(), "second mark is a no-op");
            assert!(guard.is_marked());
            assert_eq!(counter.load(Ordering::Relaxed), 1);
        }

        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "dropping the connection must release its stream slot"
        );
    }

    #[test]
    fn unmarked_guard_never_touches_the_counter() {
        let counter = Arc::new(AtomicUsize::new(0));
        drop(ActiveStreamGuard::new(Arc::clone(&counter)));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
