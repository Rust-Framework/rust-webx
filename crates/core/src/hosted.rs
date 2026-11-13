//! Background work: the `BackgroundService` worker base class, its host
//! adapter, and cancellation helpers.
//!
//! ## Why
//!
//! `IHostedService` is the startup/shutdown hook: `start()` runs to completion
//! and then the host accepts requests. A **worker** (queue consumer, outbox
//! publisher, periodic poller) instead runs *forever* next to the request
//! pipeline. Written as an `IHostedService` it has to hand-roll the loop, the
//! cancellation flag, the join handle and the shutdown logic. Business code
//! should only write the loop body:
//!
//! ```ignore
//! struct OutboxWorker { services: Arc<dyn ScopeFactory> }
//!
//! #[async_trait]
//! impl BackgroundService for OutboxWorker {
//!     async fn execute(&self, ct: CancellationToken) -> Result<()> {
//!         while !ct.is_cancelled() {
//!             let scope = self.services.create_scope();
//!             publish_due(&scope).await?;
//!             // Interruptible wait: returns `false` immediately on shutdown.
//!             if !delay(&ct, Duration::from_secs(5)).await {
//!                 break;
//!             }
//!         }
//!         Ok(())
//!     }
//! }
//! ```
//!
//! Register it with `add_background_service_with` / `add_background_service`;
//! the host starts it together with the hosted services and **cancels the token
//! on shutdown**, so the loop leaves `delay` at once instead of finishing a long
//! sleep.
//!
//! ## Failure semantics
//!
//! `execute` returning `Err` is a typed failure, not a panic: the adapter logs
//! it when it happens and reports it again from `stop()`. A panic inside
//! `execute` is caught by the task and converted to [`Error::Internal`] in
//! `stop()`. A worker that
//! ignores cancellation is aborted after [`STOP_GRACE`] so it cannot outlive
//! provider disposal.

use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::handler::IHostedService;

pub use tokio_util::sync::CancellationToken;

/// How long a background worker may take to observe cancellation.
///
/// The host itself allows 30s per `stop()`; a worker that is still running after
/// this grace period is aborted, because a detached task outliving
/// `provider.dispose()` would keep touching disposed services.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// A long-running worker: the host calls [`execute`](BackgroundService::execute)
/// once on startup and cancels the token on shutdown.
///
/// Business code only implements the loop body; scope creation, shutdown
/// observation, panic conversion and the join-handle lifecycle belong to the
/// framework (`BackgroundServiceHost`).
#[async_trait::async_trait]
pub trait BackgroundService: Send + Sync + 'static {
    /// Run until `ct` is cancelled (or the work is finished).
    ///
    /// `ct` is cancelled when the host starts shutting down. Long waits must be
    /// interruptible — use [`delay`] instead of `tokio::time::sleep`, so a
    /// shutdown does not wait for the next tick.
    async fn execute(&self, ct: CancellationToken) -> Result<()>;
}

/// Wait for `duration`, returning `false` as soon as `ct` is cancelled.
///
/// This is the interruptible counterpart of `tokio::time::sleep`: a worker that
/// sleeps for 30s stops within milliseconds of a shutdown.
///
/// ```ignore
/// while !ct.is_cancelled() {
///     work().await?;
///     if !delay(&ct, interval).await {
///         break; // cancelled
///     }
/// }
/// ```
pub async fn delay(ct: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = ct.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

/// A periodic worker: the framework owns the interval loop, the interruptible
/// wait and stop/join; business code implements [`tick`](PeriodicBackgroundService::tick)
/// only.
///
/// This is the extracted form of the hand-rolled
/// `AtomicBool + Notify + Mutex<JoinHandle>` runner every application used to
/// duplicate (interval, wake-on-stop, join, "tick already running finishes"
/// semantics). Implement it, register it with `add_background_service*`, and
/// you get a cancellable timer for free:
///
/// ```ignore
/// struct OutboxWorker { services: Arc<dyn ScopeFactory>, interval: Duration }
///
/// #[async_trait]
/// impl PeriodicBackgroundService for OutboxWorker {
///     fn interval(&self) -> Duration { self.interval }
///
///     async fn tick(&self) -> Result<()> {
///         let scope = self.services.create_scope(); // one scope per tick
///         publish_due(&scope).await
///     }
/// }
/// ```
///
/// Semantics (kept identical to the runner this replaces):
///
/// * the first tick runs **after one interval**;
/// * a tick that is already running finishes before the loop stops — keep ticks
///   short/bounded;
/// * a failed tick is logged and the next one runs on schedule, unless
///   [`stop_on_error`](PeriodicBackgroundService::stop_on_error) is `true`;
/// * shutdown interrupts the wait immediately, so a 60s interval never delays a
///   30s shutdown window.
///
/// It is automatically a [`BackgroundService`], so
/// `add_background_service_with` / `add_background_service` accept it as-is.
#[async_trait::async_trait]
pub trait PeriodicBackgroundService: Send + Sync + 'static {
    /// How long to wait between ticks.
    ///
    /// Must be greater than zero; a zero interval is a configuration error and
    /// fails the worker instead of spinning the CPU.
    fn interval(&self) -> Duration;

    /// One tick. Return `Err` to report a failure (logged, loop continues by
    /// default).
    async fn tick(&self) -> Result<()>;

    /// Whether a failed tick stops the worker.
    ///
    /// Defaults to `false`: a poller must survive a transient database outage.
    /// Return `true` for work where a failure is fatal (the error then ends
    /// `execute` and is reported from `stop()`).
    fn stop_on_error(&self) -> bool {
        false
    }
}

#[async_trait::async_trait]
impl<T: PeriodicBackgroundService + ?Sized> BackgroundService for T {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        let interval = self.interval();
        if interval.is_zero() {
            return Err(Error::Internal(
                "periodic background service interval must be greater than zero".to_string(),
            ));
        }

        // Wait first (the first tick is one interval after startup), then tick
        // until shutdown. `delay` wakes as soon as the token is cancelled.
        while delay(&ct, interval).await {
            if let Err(err) = self.tick().await {
                tracing::error!(
                    error = %err,
                    interval_secs = interval.as_secs_f64(),
                    "periodic background service tick failed"
                );
                if self.stop_on_error() {
                    return Err(err);
                }
            }
        }
        Ok(())
    }
}

/// Runs a [`BackgroundService`] as an [`IHostedService`].
///
/// Registered for you by `add_background_service*`; construct it directly only
/// when you need to compose it yourself.
pub struct BackgroundServiceHost {
    service: Arc<dyn BackgroundService>,
    run: tokio::sync::Mutex<Option<Running>>,
}

struct Running {
    stop: CancellationToken,
    handle: tokio::task::JoinHandle<Result<()>>,
}

impl BackgroundServiceHost {
    /// Wrap `service` so the host can start and stop it.
    pub fn new(service: Arc<dyn BackgroundService>) -> Self {
        Self {
            service,
            run: tokio::sync::Mutex::new(None),
        }
    }

    /// Cancel and join the worker, waiting at most `grace` for it to observe
    /// cancellation.
    ///
    /// `stop()` uses [`STOP_GRACE`]; tests use a shorter grace so exercising the
    /// abort path does not slow the suite down.
    pub(crate) async fn stop_with_grace(&self, grace: Duration) -> Result<()> {
        // Taking the state makes `stop()` idempotent and race-free with `start()`.
        let Some(running) = self.run.lock().await.take() else {
            return Ok(());
        };

        running.stop.cancel();
        let mut handle = running.handle;
        match tokio::time::timeout(grace, &mut handle).await {
            Ok(Ok(result)) => result,
            // The task was aborted or the runtime is shutting down.
            Ok(Err(join_error)) if join_error.is_cancelled() => Ok(()),
            Ok(Err(join_error)) => Err(Error::Internal(format!(
                "background service panicked: {join_error}"
            ))),
            Err(_) => {
                handle.abort();
                Err(Error::Internal(format!(
                    "background service did not observe cancellation within {}s and was aborted",
                    grace.as_secs()
                )))
            }
        }
    }
}

#[async_trait::async_trait]
impl IHostedService for BackgroundServiceHost {
    async fn start(&self) -> Result<()> {
        let mut slot = self.run.lock().await;
        if slot.is_some() {
            return Err(Error::State(
                "background service is already running".to_string(),
            ));
        }

        let stop = CancellationToken::new();
        let service = Arc::clone(&self.service);
        let token = stop.clone();
        let handle = tokio::spawn(async move {
            let result = service.execute(token).await;
            // Log at the moment of failure: `stop()` may run much later, and a
            // worker that died silently is exactly what operators miss.
            if let Err(err) = &result {
                tracing::error!(error = %err, "background service exited with an error");
            }
            result
        });

        *slot = Some(Running { stop, handle });
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stop_with_grace(STOP_GRACE).await
    }
}

/// Hosted service that starts after every registration with a lower `order`.
///
/// Implementation detail of `add_hosted_service_ordered` /
/// `add_hosted_service_with_ordered`: the host discovers hosted services through
/// DI, so the order has to travel with the instance.
pub(crate) struct OrderedHostedService<T: ?Sized> {
    order: i32,
    inner: Arc<T>,
}

impl<T: IHostedService + ?Sized> OrderedHostedService<T> {
    pub(crate) fn new(order: i32, inner: Arc<T>) -> Self {
        Self { order, inner }
    }
}

#[async_trait::async_trait]
impl<T: IHostedService + ?Sized> IHostedService for OrderedHostedService<T> {
    async fn start(&self) -> Result<()> {
        self.inner.start().await
    }

    async fn stop(&self) -> Result<()> {
        self.inner.stop().await
    }

    fn order(&self) -> i32 {
        self.order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Instant;

    struct Idle;

    #[async_trait::async_trait]
    impl BackgroundService for Idle {
        async fn execute(&self, _ct: CancellationToken) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn delay_is_interrupted_by_cancellation() {
        let ct = CancellationToken::new();
        let cancel = ct.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        });

        let started = Instant::now();
        let slept = delay(&ct, Duration::from_secs(60)).await;

        assert!(!slept, "a cancelled delay must report cancellation");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "cancellation must interrupt the sleep within 100ms, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn delay_waits_the_full_duration_without_cancellation() {
        let ct = CancellationToken::new();
        assert!(delay(&ct, Duration::from_millis(10)).await);
    }

    struct Counting(Arc<AtomicBool>);

    #[async_trait::async_trait]
    impl BackgroundService for Counting {
        async fn execute(&self, ct: CancellationToken) -> Result<()> {
            while delay(&ct, Duration::from_secs(60)).await {
                self.0.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn stop_cancels_the_worker_and_joins_it() {
        let stopped = Arc::new(AtomicBool::new(false));
        let host = BackgroundServiceHost::new(Arc::new(Counting(Arc::clone(&stopped))));

        host.start().await.expect("start must succeed");
        tokio::time::sleep(Duration::from_millis(20)).await;

        let started = Instant::now();
        host.stop().await.expect("stop must succeed");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "stop must interrupt the worker's sleep"
        );
        assert!(!stopped.load(Ordering::SeqCst), "the loop never ticked");

        // Stopping twice is a no-op, so the host may stop on shutdown and again
        // during teardown.
        host.stop().await.expect("second stop must be a no-op");
    }

    struct Failing;

    #[async_trait::async_trait]
    impl BackgroundService for Failing {
        async fn execute(&self, _ct: CancellationToken) -> Result<()> {
            Err(Error::Internal("worker exploded".to_string()))
        }
    }

    #[tokio::test]
    async fn worker_failure_is_reported_as_a_typed_error_from_stop() {
        let host = BackgroundServiceHost::new(Arc::new(Failing));
        host.start().await.expect("start must succeed");
        // Let `execute` fail before stop observes it.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let err = host
            .stop()
            .await
            .expect_err("failure must not be swallowed");
        assert!(err.to_string().contains("worker exploded"), "{err}");
    }

    struct Panicking;

    #[async_trait::async_trait]
    impl BackgroundService for Panicking {
        async fn execute(&self, _ct: CancellationToken) -> Result<()> {
            panic!("boom");
        }
    }

    #[tokio::test]
    async fn worker_panic_becomes_a_typed_error_instead_of_propagating() {
        let host = BackgroundServiceHost::new(Arc::new(Panicking));
        host.start().await.expect("start must succeed");
        tokio::time::sleep(Duration::from_millis(20)).await;

        let err = host.stop().await.expect_err("panic must be a typed error");
        assert!(
            err.to_string().contains("background service panicked"),
            "{err}"
        );
    }

    struct Stubborn;

    #[async_trait::async_trait]
    impl BackgroundService for Stubborn {
        async fn execute(&self, _ct: CancellationToken) -> Result<()> {
            // Ignores cancellation entirely.
            tokio::time::sleep(Duration::from_secs(600)).await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_worker_that_ignores_cancellation_is_aborted() {
        let host = BackgroundServiceHost::new(Arc::new(Stubborn));
        host.start().await.expect("start must succeed");

        // Shorten nothing: `stop` waits STOP_GRACE. Keep the test honest but
        // fast by only asserting that it eventually returns a typed error.
        let result = tokio::time::timeout(Duration::from_secs(10), host.stop()).await;
        let err = result
            .expect("stop must not hang forever")
            .expect_err("aborted worker is reported as an error");
        assert!(err.to_string().contains("aborted"), "{err}");
    }

    #[tokio::test]
    async fn idle_worker_stops_cleanly() {
        let host = BackgroundServiceHost::new(Arc::new(Idle));
        host.start().await.expect("start must succeed");
        tokio::time::sleep(Duration::from_millis(20)).await;
        host.stop().await.expect("a finished worker stops cleanly");
    }

    struct Ticker {
        ticks: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl PeriodicBackgroundService for Ticker {
        fn interval(&self) -> Duration {
            Duration::from_millis(10)
        }

        async fn tick(&self) -> Result<()> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn periodic_worker_ticks_until_stopped() {
        let ticks = Arc::new(AtomicU32::new(0));
        let host = BackgroundServiceHost::new(Arc::new(Ticker {
            ticks: Arc::clone(&ticks),
        }));

        host.start().await.expect("start must succeed");
        // Wait for a couple of ticks with a bound instead of a fixed sleep.
        let deadline = Instant::now() + Duration::from_secs(2);
        while ticks.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            ticks.load(Ordering::SeqCst) >= 2,
            "the periodic worker must tick, saw {}",
            ticks.load(Ordering::SeqCst)
        );

        let started = Instant::now();
        host.stop().await.expect("stop must succeed");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "stop must not wait for the next interval"
        );

        // A completed stop joins the task: no tick may run afterwards.
        let after_stop = ticks.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(ticks.load(Ordering::SeqCst), after_stop);
    }

    struct LongInterval(Arc<AtomicU32>);

    #[async_trait::async_trait]
    impl PeriodicBackgroundService for LongInterval {
        fn interval(&self) -> Duration {
            Duration::from_secs(60)
        }

        async fn tick(&self) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn periodic_worker_shutdown_interrupts_the_interval_wait() {
        let ticks = Arc::new(AtomicU32::new(0));
        let host = BackgroundServiceHost::new(Arc::new(LongInterval(Arc::clone(&ticks))));
        host.start().await.expect("start must succeed");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            ticks.load(Ordering::SeqCst),
            0,
            "first tick is one interval away"
        );

        let started = Instant::now();
        host.stop().await.expect("stop must succeed");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the 60s interval must not delay shutdown, took {:?}",
            started.elapsed()
        );
        assert_eq!(ticks.load(Ordering::SeqCst), 0);
    }

    struct FailingTicker {
        ticks: Arc<AtomicU32>,
        fatal: bool,
    }

    #[async_trait::async_trait]
    impl PeriodicBackgroundService for FailingTicker {
        fn interval(&self) -> Duration {
            Duration::from_millis(5)
        }

        async fn tick(&self) -> Result<()> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            Err(Error::Internal("tick failed".to_string()))
        }

        fn stop_on_error(&self) -> bool {
            self.fatal
        }
    }

    #[tokio::test]
    async fn a_failed_tick_is_logged_and_the_loop_continues() {
        let ticks = Arc::new(AtomicU32::new(0));
        let host = BackgroundServiceHost::new(Arc::new(FailingTicker {
            ticks: Arc::clone(&ticks),
            fatal: false,
        }));
        host.start().await.expect("start must succeed");

        let deadline = Instant::now() + Duration::from_secs(2);
        while ticks.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            ticks.load(Ordering::SeqCst) >= 3,
            "a transient failure must not stop a poller"
        );
        // Stopping after a failed tick is clean: the error was handled.
        host.stop().await.expect("stop must succeed");
    }

    #[tokio::test]
    async fn stop_on_error_surfaces_the_typed_tick_error() {
        let ticks = Arc::new(AtomicU32::new(0));
        let host = BackgroundServiceHost::new(Arc::new(FailingTicker { ticks, fatal: true }));
        host.start().await.expect("start must succeed");

        // Let `execute` fail before stop observes it.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let err = host
            .stop()
            .await
            .expect_err("a fatal tick error must be reported");
        assert!(err.to_string().contains("tick failed"), "{err}");
    }

    struct ZeroInterval;

    #[async_trait::async_trait]
    impl PeriodicBackgroundService for ZeroInterval {
        fn interval(&self) -> Duration {
            Duration::ZERO
        }

        async fn tick(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn zero_interval_is_a_typed_configuration_error() {
        let host = BackgroundServiceHost::new(Arc::new(ZeroInterval));
        host.start().await.expect("start must succeed");
        tokio::time::sleep(Duration::from_millis(20)).await;

        let err = host.stop().await.expect_err("zero interval must fail");
        assert!(err.to_string().contains("greater than zero"), "{err}");
    }
}
