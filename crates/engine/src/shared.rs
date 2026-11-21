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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{watch, OwnedSemaphorePermit};
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
    /// Set when the owner abandons `Server::run` (future dropped/aborted).
    ///
    /// Graceful shutdown still drains; this flag switches workers to aborting
    /// their remaining connections so a cancelled supervisor does not leave
    /// detached worker threads running for the stream drain budget.
    pub immediate_shutdown: Arc<AtomicBool>,
    /// Wakes workers already inside `drain_connections` when the immediate
    /// flag is raised. A `watch` (rather than `Notify`) stores the signal, so
    /// a worker that subscribes after the trigger cannot miss it.
    pub immediate_watch: watch::Sender<bool>,
    /// One-shot engine lifecycle guard: a `Shared` cannot be served twice.
    pub started: Arc<AtomicBool>,
    /// Connection admission counters (admitted / rejected / waiting).
    pub admission: Arc<AdmissionStats>,
}

/// Process-wide connection admission counters and live gauges.
///
/// `waiting` is the queue depth between "socket accepted" and "permit granted";
/// `active` is the number of admitted connections currently open (one per held
/// admission permit, including keep-alive and streaming connections);
/// `rejected` counts sockets that waited past `Server.AdmissionTimeoutMs` and
/// received a `503` instead of being left in the kernel backlog.
#[derive(Debug, Default)]
pub struct AdmissionStats {
    admitted: AtomicU64,
    rejected: AtomicU64,
    waiting: AtomicU64,
    active: AtomicUsize,
    capacity: AtomicUsize,
}

impl AdmissionStats {
    /// Connections admitted by the global governor.
    pub fn admitted(&self) -> u64 {
        self.admitted.load(Ordering::Relaxed)
    }

    /// Connections rejected after the admission timeout.
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    /// Connections currently open (holding an admission permit).
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// Configured admission capacity (`App.MaxConnections`).
    ///
    /// `0` until the host records it; the engine gauge reports it as a
    /// capacity reference so `active / capacity` is visible on one scrape.
    pub fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Record the configured admission capacity (host startup).
    pub fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, Ordering::Relaxed);
    }

    pub(crate) fn record_connection_open(&self) {
        self.active.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_connection_closed(&self) {
        // Saturating decrement: a double close must never wrap the gauge.
        let mut current = self.active.load(Ordering::Relaxed);
        while current > 0 {
            match self.active.compare_exchange_weak(
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

    /// Accepted sockets currently waiting for an admission slot.
    pub fn waiting(&self) -> u64 {
        self.waiting.load(Ordering::Relaxed)
    }

    pub(crate) fn record_admitted(&self) {
        self.admitted.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn begin_wait(&self) {
        self.waiting.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn end_wait(&self) {
        self.waiting.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Admission slot for one accepted connection.
///
/// Keeps the global governor permit for the connection's lifetime and maintains
/// [`AdmissionStats::active`]: the counter is incremented once on construction
/// and decremented by `Drop`. The permit, this guard, the worker handoff and
/// the spawned connection task are one ownership chain, so the gauge cannot
/// leak on abort, shutdown, or a dropped handoff.
#[derive(Debug)]
pub struct ConnectionPermit {
    /// Held for its `Drop` side effect (releases the governor slot).
    _permit: OwnedSemaphorePermit,
    stats: Arc<AdmissionStats>,
}

impl ConnectionPermit {
    /// Take ownership of `permit` and open one active-connection slot.
    pub fn new(permit: OwnedSemaphorePermit, stats: Arc<AdmissionStats>) -> Self {
        stats.record_connection_open();
        Self {
            _permit: permit,
            stats,
        }
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.stats.record_connection_closed();
    }
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

    /// Abandon the run: signal shutdown and make workers abort their remaining
    /// connections instead of draining for up to `stream_drain_timeout`.
    ///
    /// Used by the engine/host run guard when the caller drops the `run` future
    /// or aborts the supervising task. It is not a normal graceful-shutdown
    /// path.
    pub fn trigger_immediate_shutdown(&self) {
        self.immediate_shutdown.store(true, Ordering::Release);
        self.immediate_watch.send_replace(true);
        self.trigger_shutdown();
    }
}

/// Per-worker stream gauge.
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
        assert_send_sync::<ConnectionPermit>();
    }

    #[test]
    fn connection_permit_tracks_the_active_gauge() {
        let stats = Arc::new(AdmissionStats::default());
        let governor = Arc::new(tokio::sync::Semaphore::new(2));
        assert_eq!(stats.active(), 0);

        let first = ConnectionPermit::new(
            Arc::clone(&governor).try_acquire_owned().expect("permit"),
            Arc::clone(&stats),
        );
        let second = ConnectionPermit::new(
            Arc::clone(&governor).try_acquire_owned().expect("permit"),
            Arc::clone(&stats),
        );
        assert_eq!(stats.active(), 2);
        assert_eq!(stats.capacity(), 0, "capacity is recorded by the host");

        drop(first);
        assert_eq!(stats.active(), 1);
        drop(second);
        assert_eq!(stats.active(), 0);
        assert_eq!(governor.available_permits(), 2);
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
