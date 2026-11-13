//! WEBX-5: hosted-service registration ergonomics and background-worker
//! lifecycle.
//!
//! Covers the gaps that made every application hand-roll its own worker
//! framework:
//!
//! * `add_hosted_service_with` constructs the service **from the DI resolver**,
//!   so configuration and dependencies are injected without `Default`;
//! * `add_hosted_service_ordered` gives start/stop ordering;
//! * `add_hosted_service_if` / `add_hosted_service_with_if` register
//!   conditionally;
//! * `add_background_service_with` runs a `BackgroundService` next to the
//!   request pipeline and **cancels it on shutdown** - an in-flight `delay`
//!   returns immediately instead of sleeping through the shutdown;
//! * a failing `start()` makes `run_at` return a typed error instead of
//!   panicking;
//! * shutdown stops hosted services **before** in-flight connections drain.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_dix::IServiceResolver;
use webx_core::app::IHost;
use webx_core::error::{Error, Result};
use webx_core::handler::IHostedService;
use webx_core::hosted::{delay, BackgroundService, CancellationToken};
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::server::Host;
use webx_host::server::ServerHandle;

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Resolve a registered singleton from the type-erased resolver the
/// registration factories receive.
fn resolve<T: Send + Sync + 'static>(resolver: &dyn IServiceResolver) -> Arc<T> {
    resolver
        .get_any(std::any::type_name::<T>())
        .and_then(|service| service.downcast::<Arc<T>>().ok())
        .map(|service| Arc::clone(&*service))
        .expect("the dependency must be registered before the factory runs")
}

/// Build a host with `configure`, run it, and wait until it answers health.
///
/// The runner handle is returned so a test can await the full shutdown path
/// instead of sleeping and hoping teardown finished.
async fn spawn(
    port: u16,
    configure: impl FnOnce(webx_host::server::HostBuilder) -> webx_host::server::HostBuilder,
) -> (ServerHandle, tokio::task::JoinHandle<Result<()>>) {
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let host = configure(
        Host::builder()
            .mode(webx_core::mode::AppMode::Development)
            .no_spa(),
    )
    .build();
    let handle = host.server_handle();
    let runner = tokio::spawn(async move { host.run_at(&addr).await });

    let client = reqwest::Client::new();
    for _ in 0..150 {
        if client
            .get(format!("{base}/health/live"))
            .send()
            .await
            .is_ok()
        {
            return (handle, runner);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host never became ready");
}

// ---------------------------------------------------------------------------
// Factory registration: configuration/dependencies without `Default`
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TestConfig {
    greeting: String,
}

struct ConfigAwareService {
    config: Arc<TestConfig>,
    started: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl IHostedService for ConfigAwareService {
    async fn start(&self) -> Result<()> {
        assert_eq!(self.config.greeting, "hi");
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn factory_registration_injects_configuration_without_default() {
    let port = find_free_port();
    let started = Arc::new(AtomicBool::new(false));
    let started_for_factory = Arc::clone(&started);

    let (handle, _runner) = spawn(port, move |builder| {
        builder.register(move |svc| {
            svc.instance(Arc::new(TestConfig {
                greeting: "hi".to_string(),
            }))
            .add_hosted_service_with(move |resolver| {
                let config = resolve::<TestConfig>(resolver);
                Arc::new(ConfigAwareService {
                    config,
                    started: Arc::clone(&started_for_factory),
                })
            })
        })
    })
    .await;

    assert!(
        started.load(Ordering::SeqCst),
        "the factory-built service must start (and see its injected config)"
    );
    handle.shutdown();
}

// ---------------------------------------------------------------------------
// Startup failure is a typed error, not a panic
// ---------------------------------------------------------------------------

struct FailingStart;

#[async_trait::async_trait]
impl IHostedService for FailingStart {
    async fn start(&self) -> Result<()> {
        Err(Error::Internal("seed failed".to_string()))
    }
}

#[tokio::test]
async fn failing_start_returns_a_typed_error_instead_of_panicking() {
    let addr = format!("127.0.0.1:{}", find_free_port());
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .register(|svc| svc.add_hosted_service_with(|_| Arc::new(FailingStart)))
        .build();

    let error = host
        .run_at(&addr)
        .await
        .expect_err("a failed hosted service must fail host startup");
    assert!(
        error.to_string().contains("seed failed"),
        "the typed error must carry the service's message, got: {error}"
    );
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

struct Recorder {
    name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl IHostedService for Recorder {
    async fn start(&self) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("start:{}", self.name));
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("stop:{}", self.name));
        Ok(())
    }
}

#[tokio::test]
async fn ordered_registration_controls_start_and_stop_order() {
    let port = find_free_port();
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let (handle, runner) = spawn(port, {
        let events = Arc::clone(&events);
        move |builder| {
            builder.register(move |svc| {
                svc.add_hosted_service_with_ordered(100, {
                    let events = Arc::clone(&events);
                    move |_| {
                        Arc::new(Recorder {
                            name: "late",
                            events: Arc::clone(&events),
                        })
                    }
                })
                .add_hosted_service_with_ordered(-100, {
                    let events = Arc::clone(&events);
                    move |_| {
                        Arc::new(Recorder {
                            name: "early",
                            events: Arc::clone(&events),
                        })
                    }
                })
                .add_hosted_service_with({
                    let events = Arc::clone(&events);
                    move |_| {
                        Arc::new(Recorder {
                            name: "middle",
                            events: Arc::clone(&events),
                        })
                    }
                })
            })
        }
    })
    .await;

    handle.shutdown();
    // Awaiting the runner means teardown (all `stop()` calls) has finished.
    runner
        .await
        .expect("host task must not panic")
        .expect("clean shutdown");

    let events = events.lock().unwrap().clone();
    assert_eq!(
        events,
        vec![
            "start:early",
            "start:middle",
            "start:late",
            "stop:late",
            "stop:middle",
            "stop:early",
        ],
        "lower order starts first; stop reverses start order"
    );
}

// ---------------------------------------------------------------------------
// Conditional registration
// ---------------------------------------------------------------------------

struct Conditional {
    started: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl IHostedService for Conditional {
    async fn start(&self) -> Result<()> {
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn disabled_conditional_registration_never_starts() {
    let addr = format!("127.0.0.1:{}", find_free_port());
    let started = Arc::new(AtomicBool::new(false));
    let started_for_factory = Arc::clone(&started);

    // Build only: the service must not even be in the container.
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .register(move |svc| {
            svc.add_hosted_service_with_if(false, move |_| {
                Arc::new(Conditional {
                    started: Arc::clone(&started_for_factory),
                })
            })
        })
        .build();

    // run_at blocks until shutdown, so drive it on a task and stop it.
    let handle = host.server_handle();
    let runner = tokio::spawn(async move { host.run_at(&addr).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!started.load(Ordering::SeqCst));
    handle.shutdown();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn enabled_conditional_registration_starts() {
    let port = find_free_port();
    let started = Arc::new(AtomicBool::new(false));
    let started_for_factory = Arc::clone(&started);

    let (handle, _runner) = spawn(port, move |builder| {
        builder.register(move |svc| {
            svc.add_hosted_service_with_if(true, move |_| {
                Arc::new(Conditional {
                    started: Arc::clone(&started_for_factory),
                })
            })
        })
    })
    .await;

    assert!(started.load(Ordering::SeqCst));
    handle.shutdown();
}

// ---------------------------------------------------------------------------
// BackgroundService workers
// ---------------------------------------------------------------------------

struct LoopWorker {
    ticked: Arc<AtomicU32>,
    exited: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl BackgroundService for LoopWorker {
    async fn execute(&self, ct: CancellationToken) -> Result<()> {
        // Long, interruptible wait: shutdown must not wait for this tick.
        while delay(&ct, Duration::from_secs(30)).await {
            self.ticked.fetch_add(1, Ordering::SeqCst);
        }
        self.exited.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn background_worker_is_cancelled_promptly_on_shutdown() {
    let port = find_free_port();
    let ticked = Arc::new(AtomicU32::new(0));
    let exited = Arc::new(AtomicBool::new(false));
    let ticked_for_factory = Arc::clone(&ticked);
    let exited_for_factory = Arc::clone(&exited);

    let (handle, _runner) = spawn(port, move |builder| {
        builder.register(move |svc| {
            svc.add_background_service_with(move |_| {
                Arc::new(LoopWorker {
                    ticked: Arc::clone(&ticked_for_factory),
                    exited: Arc::clone(&exited_for_factory),
                })
            })
        })
    })
    .await;

    assert!(
        !exited.load(Ordering::SeqCst),
        "the worker must still be running while the host serves"
    );
    assert_eq!(
        ticked.load(Ordering::SeqCst),
        0,
        "30s sleep is still pending"
    );

    let started = Instant::now();
    handle.shutdown();
    // Allow the shutdown path to run and join the worker.
    for _ in 0..100 {
        if exited.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(
        exited.load(Ordering::SeqCst),
        "the worker's `delay` must return as soon as the token is cancelled"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown must not wait for the worker's 30s sleep, took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Lifecycle ordering: hosted services stop before connections drain
// ---------------------------------------------------------------------------

/// Sleeps 300ms when the request carries `?slow=1`, so one request can be
/// in flight while shutdown starts.
struct SlowOnDemand;

#[async_trait::async_trait]
impl IMiddleware for SlowOnDemand {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<std::ops::ControlFlow<()>> {
        if ctx.request().query().get("slow").is_some() {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Ok(std::ops::ControlFlow::Continue(()))
    }
}

struct StopTimeRecorder {
    stopped_at: Arc<Mutex<Option<Instant>>>,
}

#[async_trait::async_trait]
impl IHostedService for StopTimeRecorder {
    async fn start(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        *self.stopped_at.lock().unwrap() = Some(Instant::now());
        Ok(())
    }
}

#[tokio::test]
async fn hosted_services_stop_before_in_flight_requests_finish() {
    let port = find_free_port();
    let stopped_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let stopped_for_factory = Arc::clone(&stopped_at);

    let (handle, _runner) = spawn(port, move |builder| {
        builder
            .use_middleware_with(|| Arc::new(SlowOnDemand))
            .register(move |svc| {
                svc.add_hosted_service_with(move |_| {
                    Arc::new(StopTimeRecorder {
                        stopped_at: Arc::clone(&stopped_for_factory),
                    })
                })
            })
    })
    .await;

    // Start a slow request, then shut down while it is still sleeping in the
    // middleware: the hosted service must stop first, not after the drain.
    let client = reqwest::Client::new();
    let request = tokio::spawn(async move {
        let response = client
            .get(format!("http://127.0.0.1:{port}/health/live?slow=1"))
            .send()
            .await;
        (response.is_ok(), Instant::now())
    });

    tokio::time::sleep(Duration::from_millis(120)).await;
    handle.shutdown();

    let (_, request_finished_at) = request.await.unwrap();
    let stopped_at = stopped_at
        .lock()
        .unwrap()
        .expect("the hosted service must be stopped on shutdown");

    assert!(
        stopped_at < request_finished_at,
        "hosted services must stop before in-flight requests drain \
         (stopped {stopped_at:?}, request finished {request_finished_at:?})"
    );
}

// ---------------------------------------------------------------------------
// PeriodicBackgroundService: business writes `tick` only
// ---------------------------------------------------------------------------

struct TickWorker {
    ticks: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl webx_core::hosted::PeriodicBackgroundService for TickWorker {
    fn interval(&self) -> Duration {
        Duration::from_millis(10)
    }

    async fn tick(&self) -> Result<()> {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn periodic_worker_runs_from_a_factory_registration() {
    let port = find_free_port();
    let ticks = Arc::new(AtomicU32::new(0));
    let ticks_for_factory = Arc::clone(&ticks);

    let (handle, _runner) = spawn(port, move |builder| {
        builder.register(move |svc| {
            svc.add_background_service_with(move |_| {
                Arc::new(TickWorker {
                    ticks: Arc::clone(&ticks_for_factory),
                })
            })
        })
    })
    .await;

    let deadline = Instant::now() + Duration::from_secs(2);
    while ticks.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        ticks.load(Ordering::SeqCst) >= 2,
        "the periodic worker must tick while the host serves"
    );

    let started = Instant::now();
    handle.shutdown();

    // Shutdown cancels the worker; a 10ms interval would keep incrementing the
    // counter if the loop survived, so a frozen count proves cancellation.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let frozen = ticks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        frozen,
        "shutdown must stop the periodic worker"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown must not wait for the next interval"
    );
}

// ---------------------------------------------------------------------------
// Existing Default-based registration keeps working
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LegacyDefaultService;

static LEGACY_STARTED: AtomicBool = AtomicBool::new(false);

#[async_trait::async_trait]
impl IHostedService for LegacyDefaultService {
    async fn start(&self) -> Result<()> {
        LEGACY_STARTED.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn add_hosted_service_with_default_still_works() {
    let port = find_free_port();
    let (handle, _runner) = spawn(port, |builder| {
        builder.register(|svc| svc.add_hosted_service::<LegacyDefaultService>())
    })
    .await;

    assert!(LEGACY_STARTED.load(Ordering::SeqCst));
    handle.shutdown();
}

// ---------------------------------------------------------------------------
// `IHost::stop()` path also cancels workers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ihost_stop_cancels_background_workers() {
    let port = find_free_port();
    let exited = Arc::new(AtomicBool::new(false));
    let exited_for_factory = Arc::clone(&exited);
    let addr = format!("127.0.0.1:{port}");

    let host = Arc::new(
        Host::builder()
            .mode(webx_core::mode::AppMode::Development)
            .no_spa()
            .register(move |svc| {
                svc.add_background_service_with(move |_| {
                    Arc::new(LoopWorker {
                        ticked: Arc::new(AtomicU32::new(0)),
                        exited: Arc::clone(&exited_for_factory),
                    })
                })
            })
            .build(),
    );

    let runner = Arc::clone(&host);
    let server = tokio::spawn(async move { runner.run_at(&addr).await });

    tokio::time::sleep(Duration::from_millis(300)).await;
    host.stop().await.unwrap();

    server.await.unwrap().unwrap();
    assert!(
        exited.load(Ordering::SeqCst),
        "IHost::stop must funnel through the same shutdown path"
    );
}
