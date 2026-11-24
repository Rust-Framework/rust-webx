//! End-to-end load probe: framework pipeline vs a bare hyper baseline.
//!
//! `router_bench` / `pipeline_bench` measure components in isolation and
//! `alloc-probe` counts allocations; neither answers "what does a real request
//! over a real socket cost, and how much of that is the framework?". This probe
//! runs two servers in one process on loopback — the `rust-webx` app below and a
//! hand-written hyper service that answers the same bytes — drives both with the
//! same keep-alive client and the same connection count, and prints throughput
//! plus latency percentiles side by side.
//!
//! ```text
//! cargo run -p load-probe --release -- --connections 8 --seconds 5
//! ```
//!
//! Reading the numbers:
//!
//! * both servers share this process's CPU, so absolute RPS is a lower bound for
//!   what a dedicated machine does; the **ratio** between the two rows is the
//!   portable number (it is the framework pipeline: router + middleware +
//!   binding + serde + error writer + response assembly);
//! * `hello` is a static-route GET returning a 32-byte JSON body (`serde`);
//! * `echo` is a POST with a ~1 KiB JSON body that is deserialized into a model
//!   and re-serialized — this is the path where binding and serde costs show up;
//! * the baseline answers `hello` with the same bytes and `echo` by echoing the
//!   raw body (no routing, no binding, no serde), so the `echo` delta includes
//!   binding/serialization by construction. It is the worst-case tax, not a bug.

use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use webx::*;

// ---------------------------------------------------------------------------
// rust-webx application under test
// ---------------------------------------------------------------------------

#[derive(Default, WebxRequestMeta)]
struct HelloRequest;

#[derive(serde::Serialize)]
struct HelloModel {
    ok: bool,
    app: &'static str,
}

#[get("/api/hello")]
impl IRequest<HelloModel> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloModel> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> ActionResult<HelloModel> {
        Ok(HelloModel {
            ok: true,
            app: "rust-webx",
        })
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
struct EchoModel {
    label: String,
    items: Vec<u64>,
}

#[derive(serde::Deserialize, WebxRequestMeta)]
struct EchoRequest {
    #[frombody]
    body: EchoModel,
}

#[post("/api/echo")]
impl IRequest<EchoModel> for EchoRequest {}

#[derive(Default)]
struct EchoHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<EchoRequest, EchoModel> for EchoHandler {
    async fn handle(&mut self, req: EchoRequest) -> ActionResult<EchoModel> {
        Ok(req.body)
    }
}

fn build_host() -> Host {
    Host::builder().mode(AppMode::Development).no_spa().build()
}

// ---------------------------------------------------------------------------
// bare hyper baseline
// ---------------------------------------------------------------------------

async fn hyper_baseline(stream: tokio::net::TcpStream) -> std::io::Result<()> {
    let service = service_fn(|req: Request<Incoming>| async move {
        let path = req.uri().path().to_owned();
        let response = match path.as_str() {
            "/api/hello" => Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from_static(
                    br#"{"ok":true,"app":"rust-webx"}"#,
                )))
                .unwrap(),
            "/api/echo" => {
                let body = req.into_body().collect().await.map(|c| c.to_bytes());
                match body {
                    Ok(bytes) => Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(Full::new(bytes))
                        .unwrap(),
                    Err(_) => Response::builder()
                        .status(400)
                        .body(Full::new(Bytes::from_static(b"{}")))
                        .unwrap(),
                }
            }
            _ => Response::builder()
                .status(404)
                .body(Full::new(Bytes::from_static(b"{}")))
                .unwrap(),
        };
        Ok::<_, std::convert::Infallible>(response)
    });
    hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await
        .map_err(std::io::Error::other)
}

async fn start_hyper_baseline(port: u16) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind baseline");
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = hyper_baseline(stream).await;
            });
        }
    });
}

// ---------------------------------------------------------------------------
// load driver
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Scenario {
    Hello,
    Echo,
}

impl Scenario {
    fn path(self) -> &'static str {
        match self {
            Scenario::Hello => "/api/hello",
            Scenario::Echo => "/api/echo",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Scenario::Hello => "hello (GET, static route, 32 B JSON)",
            Scenario::Echo => "echo (POST, 1 KiB JSON body, serde)",
        }
    }
}

fn echo_payload() -> Bytes {
    let model = EchoModel {
        label: "load-probe".to_string(),
        items: (0..128u64).collect(),
    };
    Bytes::from(serde_json::to_vec(&model).expect("serialize payload"))
}

/// Per-request latencies in microseconds plus the measured window.
struct Stats {
    latencies: Vec<u64>,
    elapsed: Duration,
}

impl Stats {
    fn rps(&self) -> f64 {
        self.latencies.len() as f64 / self.elapsed.as_secs_f64()
    }

    fn percentile(&self, p: f64) -> u64 {
        if self.latencies.is_empty() {
            return 0;
        }
        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[index]
    }
}

async fn drive(
    client: Client<HttpConnector, Full<Bytes>>,
    base: String,
    scenario: Scenario,
    connections: usize,
    duration: Duration,
) -> Stats {
    let deadline = Instant::now() + duration;
    let payload = echo_payload();
    let mut tasks = Vec::with_capacity(connections);
    for _ in 0..connections {
        let client = client.clone();
        let base = base.clone();
        let payload = payload.clone();
        tasks.push(tokio::spawn(async move {
            let mut latencies = Vec::with_capacity(16 * 1024);
            while Instant::now() < deadline {
                let mut request = Request::builder()
                    .method(match scenario {
                        Scenario::Hello => "GET",
                        Scenario::Echo => "POST",
                    })
                    .uri(format!("{base}{}", scenario.path()))
                    .header("accept", "application/json");
                let body = match scenario {
                    Scenario::Hello => Full::new(Bytes::new()),
                    Scenario::Echo => {
                        request = request.header("content-type", "application/json");
                        Full::new(payload.clone())
                    }
                };
                let request = request.body(body).expect("build request");
                let started = Instant::now();
                match client.request(request).await {
                    Ok(response) => {
                        if response.into_body().collect().await.is_err() {
                            break;
                        }
                        latencies.push(started.elapsed().as_micros() as u64);
                    }
                    Err(_) => break,
                }
            }
            latencies
        }));
    }

    let mut latencies = Vec::new();
    for task in tasks {
        if let Ok(mut latencies_of_task) = task.await {
            latencies.append(&mut latencies_of_task);
        }
    }
    Stats {
        latencies,
        elapsed: duration,
    }
}

fn print_row(label: &str, stats: &Stats) {
    println!(
        "{:<34} {:>10.0} {:>8} {:>8} {:>8} {:>9}",
        label,
        stats.rps(),
        stats.percentile(0.50),
        stats.percentile(0.90),
        stats.percentile(0.99),
        stats.latencies.iter().copied().max().unwrap_or(0),
    );
}

fn print_table(scenario: Scenario, framework: &Stats, baseline: &Stats) {
    println!();
    println!("scenario: {}", scenario.name());
    println!(
        "{:<34} {:>10} {:>8} {:>8} {:>8} {:>9}",
        "", "req/s", "p50", "p90", "p99", "max"
    );
    println!("{}", "-".repeat(81));
    print_row("rust-webx (Host::builder)", framework);
    print_row("bare hyper 1.x (no framework)", baseline);
    println!("{}", "-".repeat(81));
    let tax_p50 =
        (framework.percentile(0.50) as f64 / baseline.percentile(0.50).max(1) as f64 - 1.0) * 100.0;
    let rps_ratio = framework.rps() / baseline.rps().max(1.0) * 100.0;
    println!(
        "framework tax: p50 {:+.0}%, throughput {:.0}% of baseline",
        tax_p50, rps_ratio
    );
}

#[tokio::main]
async fn main() {
    let mut connections = 8usize;
    let mut seconds = 5u64;
    let args: Vec<String> = std::env::args().collect();
    let mut index = 1;
    while index + 1 < args.len() {
        match args[index].as_str() {
            "--connections" => {
                connections = args[index + 1].parse().expect("--connections <usize>")
            }
            "--seconds" => seconds = args[index + 1].parse().expect("--seconds <u64>"),
            "--help" | "-h" => {
                println!("usage: load-probe [--connections N] [--seconds N]");
                return;
            }
            other => panic!("unknown argument {other}"),
        }
        index += 2;
    }

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    println!(
        "load-probe: {connections} keep-alive connections, {seconds}s per row, {threads} logical CPUs, loopback"
    );

    let framework_port = free_port();
    let baseline_port = free_port();
    let framework = spawn(build_host(), framework_port).await;
    start_hyper_baseline(baseline_port).await;
    let baseline_base = format!("http://127.0.0.1:{baseline_port}");

    let client: Client<HttpConnector, Full<Bytes>> =
        Client::builder(TokioExecutor::new()).build_http();

    for scenario in [Scenario::Hello, Scenario::Echo] {
        // Warm both sides up: keep-alive setup and lazy initialisation must not
        // land inside the measured window.
        let _ = drive(
            client.clone(),
            framework.base_url.clone(),
            scenario,
            connections,
            Duration::from_millis(500),
        )
        .await;
        let _ = drive(
            client.clone(),
            baseline_base.clone(),
            scenario,
            connections,
            Duration::from_millis(500),
        )
        .await;

        let framework_stats = drive(
            client.clone(),
            framework.base_url.clone(),
            scenario,
            connections,
            Duration::from_secs(seconds),
        )
        .await;
        let baseline_stats = drive(
            client.clone(),
            baseline_base.clone(),
            scenario,
            connections,
            Duration::from_secs(seconds),
        )
        .await;
        print_table(scenario, &framework_stats, &baseline_stats);
    }

    framework.teardown().await;
}
