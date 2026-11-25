//! Allocation probe for the rust-webx hot path.
//!
//! A development tool (never published): it counts process-wide allocations in
//! a **server process** and reports allocations per `/hello` request. The
//! load-generating client runs in a separate process, so its own allocations
//! never reach the server's counter.
//!
//! ```text
//! cargo run -p alloc-probe                 # measure on the default port
//! cargo run -p alloc-probe -- 18471 1000   # port + request count
//! cargo run -p alloc-probe -- serve 18471  # server mode (spawned for you)
//! ```
//!
//! Why a separate crate: the framework forbids `unsafe`, and a counting
//! `GlobalAlloc` requires it. Keeping the probe out of the framework crates
//! preserves that guarantee.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

/// Allocations (`alloc` + `realloc`) observed by this process.
static ALLOCS: AtomicU64 = AtomicU64::new(0);
/// `/hello` requests served by this process.
static REQUESTS: AtomicU64 = AtomicU64::new(0);

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// A no-op middleware used to separate per-middleware pipeline cost from the
/// authentication work itself (`ALLOC_PROBE_NOOP_MW=1`).
#[derive(Default)]
struct NoopMiddleware;

#[async_trait::async_trait]
impl IMiddleware for NoopMiddleware {
    async fn invoke(&self, _ctx: &mut dyn webx::IHttpContext) -> Result<std::ops::ControlFlow<()>> {
        Ok(std::ops::ControlFlow::Continue(()))
    }
}

// ---------------------------------------------------------------------------
// Routes served by the probe
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct HelloRequest;

#[derive(Serialize)]
struct HelloView {
    message: &'static str,
}

#[get("/hello")]
impl IRequest<HelloView> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloView> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> ActionResult<HelloView> {
        REQUESTS.fetch_add(1, Ordering::Relaxed);
        Ok(HelloView {
            message: "Hello, World!",
        })
    }
}

#[derive(Default, WebxRequestMeta)]
struct StatsRequest;

#[derive(Serialize, Deserialize)]
struct StatsView {
    allocs: u64,
    requests: u64,
}

#[get("/__stats")]
impl IRequest<StatsView> for StatsRequest {}

#[derive(Default)]
struct StatsHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StatsRequest, StatsView> for StatsHandler {
    async fn handle(&mut self, _req: StatsRequest) -> ActionResult<StatsView> {
        Ok(StatsView {
            allocs: ALLOCS.load(Ordering::Relaxed),
            requests: REQUESTS.load(Ordering::Relaxed),
        })
    }
}

// ---------------------------------------------------------------------------
// Server mode
// ---------------------------------------------------------------------------

fn serve(port: u16) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("probe runtime");
    let address = format!("127.0.0.1:{port}");
    let mut builder = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                // One worker keeps the measurement window deterministic; the
                // default middleware chain stays on so the number reflects the
                // real hot path.
                options.server.workers = 1;
                options.metrics.enabled = false;
                // The probe measures the unthrottled hot path. Keep any
                // environment/config-provided rate limit from shaping the
                // measurement window (CI runs may carry a Production overlay).
                options.rate_limit.enabled = false;
                options.rate_limit.requests_per_second = 1_000_000.0;
                options.rate_limit.burst_size = 1_000_000;
                // A stable strong secret is required only when the auth
                // middleware is enabled; the default issuer never runs here.
                options.jwt.secret = "alloc-probe-jwt-secret-at-least-32-bytes-long".to_string();
                // `ALLOC_PROBE_NO_CORS=1` removes the CORS middleware so its
                // contribution can be measured (Production forbids the default
                // wildcard origin).
                options.cors.origins = if std::env::var_os("ALLOC_PROBE_NO_CORS").is_some() {
                    Vec::new()
                } else {
                    vec!["https://alloc-probe.invalid".to_string()]
                };
            })
        });
    // `ALLOC_PROBE_AUTH=1` measures the anonymous authentication path in
    // isolation: run `compare` twice (with and without this variable) and
    // subtract the framework numbers.
    if std::env::var_os("ALLOC_PROBE_AUTH").is_some() {
        builder = builder.use_authentication();
    }
    if std::env::var_os("ALLOC_PROBE_NOOP_MW").is_some() {
        builder = builder.use_middleware(MiddlewareSpec::new::<NoopMiddleware>());
    }
    let host = builder.build();
    runtime.block_on(async move {
        host.run_at(&address).await.expect("probe server");
    });
}

// ---------------------------------------------------------------------------
// Measurement mode
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => {
            let port = args
                .get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(18471);
            serve(port);
        }
        Some("serve-baseline") => {
            let port = args
                .get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(18471);
            baseline_serve(port);
        }
        Some("compare") => {
            let requests = args
                .get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(500);
            let gate = args.get(3).and_then(|value| value.parse::<f64>().ok());
            compare(requests, gate);
        }
        other => {
            let port = other.and_then(|value| value.parse().ok()).unwrap_or(18471);
            let requests = args
                .get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(500);
            let baseline = args.get(3).is_some_and(|mode| mode == "baseline");
            measure(port, requests, baseline);
        }
    }
}

/// Run both modes on the same machine and compare the framework against the
/// hyper floor. Exits non-zero when `gate` is given and exceeded.
fn compare(requests: u64, gate: Option<f64>) {
    let framework = measure_once(18471, requests, false);
    let baseline = measure_once(18472, requests, true);
    let tax = framework.0 - baseline.0;
    println!(
        "framework={:.2} baseline={:.2} tax={:.2} requests={}/{}",
        framework.0, baseline.0, tax, framework.1, baseline.1
    );
    if let Some(limit) = gate {
        if tax > limit {
            eprintln!("allocation budget exceeded: framework tax {tax:.2} > {limit:.2}");
            std::process::exit(1);
        }
        println!("allocation budget ok: framework tax {tax:.2} <= {limit:.2}");
    }
}

/// Bare hyper server with the same response body: the allocation floor the
/// framework cannot go below (HTTP parsing + response write).
fn baseline_serve(port: u16) {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("baseline runtime");
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("baseline bind");
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    eprintln!("baseline accept failed: {error}");
                    continue;
                }
            };
            tokio::spawn(async move {
                let service = service_fn(|request: Request<Incoming>| async move {
                    let (status, body) = match request.uri().path() {
                        "/hello" => {
                            REQUESTS.fetch_add(1, Ordering::Relaxed);
                            (200, Bytes::from_static(br#"{"message":"Hello, World!"}"#))
                        }
                        "/__stats" => {
                            let body = format!(
                                "{{\"allocs\":{},\"requests\":{}}}",
                                ALLOCS.load(Ordering::Relaxed),
                                REQUESTS.load(Ordering::Relaxed)
                            );
                            (200, Bytes::from(body))
                        }
                        _ => (404, Bytes::new()),
                    };
                    let length = body.len().to_string();
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .header("content-length", length)
                            .body(Full::new(body))
                            .expect("baseline response"),
                    )
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
}

fn measure(port: u16, requests: u64, baseline: bool) {
    let (per_request, served) = measure_once(port, requests, baseline);
    println!(
        "mode={} requests={served} allocs/request={per_request:.2}",
        if baseline {
            "hyper-baseline"
        } else {
            "framework"
        }
    );
}

/// Measure one mode and return `(allocs per request, requests served)`.
fn measure_once(port: u16, requests: u64, baseline: bool) -> (f64, u64) {
    let executable = std::env::current_exe().expect("probe executable");
    let mode = if baseline { "serve-baseline" } else { "serve" };
    let mut server = Command::new(executable)
        .args([mode, &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn probe server");
    wait_until_ready(port);

    let mut connection = TcpStream::connect(("127.0.0.1", port)).expect("probe connection");
    connection
        .set_nodelay(true)
        .expect("TCP_NODELAY on probe connection");

    // Warm up routing/JIT-free paths before the counted window.
    for _ in 0..50 {
        send(&mut connection, "/hello");
    }

    // Fail loudly with the actual body when route registration is missing
    // (LTO/linker differences can otherwise surface as a confusing stats
    // deserialization error much later).
    let hello = send(&mut connection, "/hello");
    assert!(
        hello.windows(13).any(|window| window == b"Hello, World!"),
        "framework /hello route did not return the expected body: {}",
        String::from_utf8_lossy(&hello)
    );

    let before = stats(&mut connection);
    for _ in 0..requests {
        send(&mut connection, "/hello");
    }
    let after = stats(&mut connection);

    server.kill().ok();
    server.wait().ok();

    let served = after.requests.saturating_sub(before.requests);
    // The `after` stats request is one extra request inside the window.
    let divisor = (served + 1).max(1) as f64;
    let allocs = after.allocs.saturating_sub(before.allocs) as f64;
    (allocs / divisor, served)
}

fn wait_until_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("probe server did not become ready");
}

fn stats(connection: &mut TcpStream) -> StatsView {
    let body = send(connection, "/__stats");
    serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "stats body: {error}; raw response body: {}",
            String::from_utf8_lossy(&body)
        )
    })
}

fn send(connection: &mut TcpStream, path: &str) -> Vec<u8> {
    let request = format!("GET {path} HTTP/1.1\r\nHost: alloc-probe\r\n\r\n");
    connection.write_all(request.as_bytes()).expect("write");
    read_response(connection)
}

/// Read exactly one keep-alive response (headers + `Content-Length` body).
fn read_response(connection: &mut TcpStream) -> Vec<u8> {
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        connection.read_exact(&mut byte).expect("read header");
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut content_length = 0usize;
    for line in head.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    connection.read_exact(&mut body).expect("read body");
    body
}
