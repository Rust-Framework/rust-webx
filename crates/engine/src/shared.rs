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

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
#[derive(Debug)]
pub struct AdmissionStats {
    admitted: AtomicU64,
    rejected: AtomicU64,
    waiting: AtomicU64,
    active: AtomicUsize,
    capacity: AtomicUsize,
    shutting_down: AtomicBool,
    /// Live protocol-upgrade sessions (WebSocket today), observed during drain.
    ///
    /// Detached from the connection `JoinSet`: hyper resolves the connection
    /// future after the upgrade handoff, so a worker that only joins its
    /// connections can drop the runtime before the session flushes its
    /// graceful-close frame. Workers wait for this count to reach zero within
    /// the drain budget.
    upgrade_sessions: UpgradeSessionGauge,
    /// Global connection governor shared by every listener/transport backend.
    ///
    /// Created lazily so a host that only runs a non-hyper transport still gets
    /// the same `App.MaxConnections` pool and the same admitted/rejected/active
    /// counters as the TCP acceptors. Wrapped in a mutex so the public
    /// `AdmissionStats` keeps its historical `UnwindSafe`/`RefUnwindSafe` auto
    /// traits (`tokio::sync::Semaphore` is not unwind-safe).
    governor: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    /// Per-transport listener counters, keyed by `IHttpTransport::kind`.
    ///
    /// Populated lazily by `Server::run_transport`; `/metrics` renders one set of
    /// series per registered transport (`transport="h3"`, ...).
    transports: Mutex<BTreeMap<&'static str, Arc<TransportMetrics>>>,
}

/// `watch` sender used as a race-free counter for detached upgrade sessions.
struct UpgradeSessionGauge(watch::Sender<usize>);

// `watch::Sender` internally holds a lock-protected cell that is not
// `RefUnwindSafe`; the gauge only counts sessions and carries no unwind-sensitive
// data, so the historical `AdmissionStats: UnwindSafe + RefUnwindSafe` contract is
// preserved explicitly.
impl std::panic::UnwindSafe for UpgradeSessionGauge {}
impl std::panic::RefUnwindSafe for UpgradeSessionGauge {}

impl std::fmt::Debug for UpgradeSessionGauge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("UpgradeSessionGauge")
    }
}

impl Default for AdmissionStats {
    fn default() -> Self {
        Self {
            admitted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            waiting: AtomicU64::new(0),
            active: AtomicUsize::new(0),
            capacity: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
            upgrade_sessions: UpgradeSessionGauge(watch::channel(0).0),
            governor: Mutex::new(None),
            transports: Mutex::new(BTreeMap::new()),
        }
    }
}

/// Live transport listener counters (one instance per `IHttpTransport::kind`).
///
/// Registered lazily by `Server::run_transport`; the listener backend records
/// connections/requests through the handle exposed on
/// [`crate::TransportRuntime::transport_metrics`].
#[derive(Debug, Default)]
pub struct TransportMetrics {
    connections_active: AtomicUsize,
    connections_total: AtomicU64,
    requests_total: AtomicU64,
    rejected_total: AtomicU64,
    handshake_failures: AtomicU64,
}

/// Read-only sample of one transport's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TransportStats {
    /// Connections currently open.
    pub active: usize,
    /// Connections opened since the transport started.
    pub connections: u64,
    /// Requests served through the neutral port.
    pub requests: u64,
    /// Connections rejected by engine admission.
    pub rejected: u64,
    /// Transport handshakes that failed before a connection existed.
    pub handshake_failures: u64,
}

impl TransportMetrics {
    /// Record one opened connection and return its active-connection guard.
    pub fn connection_opened(self: &Arc<Self>) -> TransportConnectionGuard {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        self.connections_active.fetch_add(1, Ordering::Relaxed);
        TransportConnectionGuard {
            metrics: Arc::clone(self),
        }
    }

    /// Record one request served through the neutral engine port.
    pub fn record_request(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one connection rejected by the engine's admission governor.
    pub fn record_rejected(&self) {
        self.rejected_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one transport handshake failure.
    pub fn record_handshake_failure(&self) {
        self.handshake_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// One consistent read of every counter.
    pub fn snapshot(&self) -> TransportStats {
        TransportStats {
            active: self.connections_active.load(Ordering::Relaxed),
            connections: self.connections_total.load(Ordering::Relaxed),
            requests: self.requests_total.load(Ordering::Relaxed),
            rejected: self.rejected_total.load(Ordering::Relaxed),
            handshake_failures: self.handshake_failures.load(Ordering::Relaxed),
        }
    }
}

/// Keeps a transport's active-connection gauge accurate for one connection.
#[derive(Debug)]
pub struct TransportConnectionGuard {
    metrics: Arc<TransportMetrics>,
}

impl Drop for TransportConnectionGuard {
    fn drop(&mut self) {
        self.metrics
            .connections_active
            .fetch_sub(1, Ordering::Relaxed);
    }
}

impl AdmissionStats {
    /// Register (or look up) the counters for one transport kind.
    pub fn register_transport(&self, kind: &'static str) -> Arc<TransportMetrics> {
        let mut transports = self
            .transports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            transports
                .entry(kind)
                .or_insert_with(|| Arc::new(TransportMetrics::default())),
        )
    }

    /// Snapshot every registered transport, ordered by kind.
    pub fn transport_snapshots(&self) -> Vec<(&'static str, TransportStats)> {
        self.transports
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(kind, metrics)| (*kind, metrics.snapshot()))
            .collect()
    }

    /// The process-wide connection governor, created on first use.
    ///
    /// Every transport backend and the TCP acceptors share this one semaphore,
    /// so `App.MaxConnections` is a connection budget, not a per-listener
    /// multiplication.
    pub(crate) fn governor(&self, max_connections: usize) -> Arc<tokio::sync::Semaphore> {
        let mut governor = self
            .governor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(governor.get_or_insert_with(|| {
            self.capacity.store(max_connections, Ordering::Relaxed);
            Arc::new(tokio::sync::Semaphore::new(max_connections))
        }))
    }

    /// Count one more live protocol-upgrade session.
    pub(crate) fn track_upgrade_session(&self) -> UpgradeSessionGuard {
        self.upgrade_sessions.0.send_modify(|count| {
            *count = count.saturating_add(1);
        });
        UpgradeSessionGuard(watch::Sender::clone(&self.upgrade_sessions.0))
    }

    /// A receiver that observes the live protocol-upgrade session count.
    pub(crate) fn subscribe_upgrade_sessions(&self) -> watch::Receiver<usize> {
        self.upgrade_sessions.0.subscribe()
    }
}

/// Decrements the live upgrade-session count on drop.
pub(crate) struct UpgradeSessionGuard(watch::Sender<usize>);

impl Drop for UpgradeSessionGuard {
    fn drop(&mut self) {
        self.0.send_modify(|count| {
            *count = count.saturating_sub(1);
        });
    }
}

/// Read-only sample of the process-wide admission state.
///
/// Returned by [`AdmissionStats::snapshot`]; a renderer or diagnostics hook
/// reads each atomic once instead of issuing a separate load per rendered field.
///
/// The type is `#[non_exhaustive]` and all fields are public: a later gauge can
/// be added without a breaking change, and tests may start from
/// [`ServerStats::default`] and set the fields they need.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServerStats {
    /// Connections admitted by the global governor.
    pub admitted: u64,
    /// Connections rejected after the admission timeout.
    pub rejected: u64,
    /// Connections currently open (holding an admission permit).
    pub active: usize,
    /// Accepted sockets currently waiting for an admission slot.
    pub waiting: u64,
    /// Configured admission capacity (`0` until the host records it).
    pub capacity: usize,
    /// `true` once graceful shutdown started.
    pub shutting_down: bool,
}

impl ServerStats {
    /// Number of internal admission invariants this sample violates.
    ///
    /// Two invariants define a healthy governor:
    ///
    /// * `active <= capacity` whenever a capacity is configured — a permit can
    ///   only be held after the global governor granted it, so more open
    ///   connections than the configured cap means the permit accounting leaked;
    /// * `waiting == 0` after graceful shutdown — the acceptor ends every
    ///   admission wait before it stops, so a leftover waiter means a wait
    ///   reached the engine without a matching release.
    ///
    /// `0` is healthy. A non-zero value is a framework bug: `/metrics` renders
    /// it as `engine_admission_invariant_violations` and the host logs it once.
    pub fn invariant_violations(&self) -> u64 {
        let over_capacity = self.capacity > 0 && self.active > self.capacity;
        let waiting_after_shutdown = self.shutting_down && self.waiting != 0;
        u64::from(over_capacity) + u64::from(waiting_after_shutdown)
    }
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

    /// Whether graceful shutdown already started.
    ///
    /// Stored by [`Shared::trigger_shutdown`], i.e. the same path the host uses
    /// to stop the engine; the flag lets `/metrics` tell "no waiters because
    /// nobody is connecting" apart from "no waiters after shutdown".
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed)
    }

    /// One read-only sample of every counter and gauge.
    pub fn snapshot(&self) -> ServerStats {
        ServerStats {
            admitted: self.admitted(),
            rejected: self.rejected(),
            active: self.active(),
            waiting: self.waiting(),
            capacity: self.capacity(),
            shutting_down: self.is_shutting_down(),
        }
    }

    pub(crate) fn record_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
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
        self.admission.record_shutdown();
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

    #[test]
    fn snapshot_reads_every_counter_and_tracks_shutdown() {
        let stats = AdmissionStats::default();
        assert_eq!(stats.snapshot(), ServerStats::default());

        stats.record_admitted();
        stats.record_admitted();
        stats.record_rejected();
        stats.begin_wait();
        stats.set_capacity(4);

        let sample = stats.snapshot();
        assert_eq!(sample.admitted, 2);
        assert_eq!(sample.rejected, 1);
        assert_eq!(sample.waiting, 1);
        assert_eq!(sample.active, 0);
        assert_eq!(sample.capacity, 4);
        assert!(!sample.shutting_down);
        assert_eq!(sample.invariant_violations(), 0);

        stats.end_wait();
        stats.record_shutdown();
        let after = stats.snapshot();
        assert!(after.shutting_down);
        assert_eq!(after.waiting, 0);
        assert_eq!(after.invariant_violations(), 0);
    }

    #[test]
    fn invariant_violations_flag_over_capacity_and_stuck_waiters() {
        let healthy = ServerStats {
            capacity: 2,
            active: 2,
            ..ServerStats::default()
        };
        assert_eq!(healthy.invariant_violations(), 0);

        // A permit leak: more open connections than the governor can grant.
        let over_capacity = ServerStats {
            capacity: 2,
            active: 3,
            ..ServerStats::default()
        };
        assert_eq!(over_capacity.invariant_violations(), 1);

        // Capacity 0 means "not configured": the gauge has no reference, so the
        // capacity invariant is not observable (and must not false-positive).
        let unknown_capacity = ServerStats {
            active: 3,
            ..ServerStats::default()
        };
        assert_eq!(unknown_capacity.invariant_violations(), 0);

        // An admission wait that outlived graceful shutdown.
        let stuck_waiter = ServerStats {
            waiting: 1,
            shutting_down: true,
            ..ServerStats::default()
        };
        assert_eq!(stuck_waiter.invariant_violations(), 1);

        let both = ServerStats {
            capacity: 1,
            active: 2,
            waiting: 1,
            shutting_down: true,
            ..ServerStats::default()
        };
        assert_eq!(both.invariant_violations(), 2);
    }
}
