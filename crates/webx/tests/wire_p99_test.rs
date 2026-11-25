//! Wire-level latency budget (2.4-8): p50/p99 over a keep-alive localhost
//! connection, asserting the framework does not regress the request path.
//!
//! Ignored by default; `stability.yml` runs it in release mode nightly with
//! `WIRE_P99_BUDGET_MS=5`. Overall runtime is a few seconds.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[derive(Default, WebxRequestMeta)]
struct HelloRequest;

#[derive(serde::Serialize)]
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
        Ok(HelloView {
            message: "Hello, World!",
        })
    }
}

fn budget_ms() -> f64 {
    std::env::var("WIRE_P99_BUDGET_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(50.0)
}

fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("perf server did not become ready");
}

fn read_response(stream: &mut TcpStream) -> Duration {
    let started = Instant::now();
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).expect("response head");
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head);
    let content_length = head
        .split("\r\n")
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).expect("response body");
    }
    started.elapsed()
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

#[test]
#[ignore = "perf: nightly CI"]
fn wire_latency_p99_stays_within_budget() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("perf runtime");
    runtime.spawn(async move {
        let host = Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .configure(|app| {
                app.options(|options| {
                    options.server.workers = 1;
                    options.metrics.enabled = false;
                })
            })
            .build();
        let _ = host.run_at(&address).await;
    });
    wait_ready(port);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connection");
    stream.set_nodelay(true).expect("nodelay");
    let request = b"GET /hello HTTP/1.1\r\nHost: perf\r\n\r\n";

    // Warm up connection setup, routing and the response encoder.
    for _ in 0..100 {
        stream.write_all(request).expect("warmup write");
        read_response(&mut stream);
    }

    const SAMPLES: usize = 500;
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        stream.write_all(request).expect("request write");
        samples.push(read_response(&mut stream));
    }
    samples.sort_unstable();

    let p50 = percentile(&samples, 0.50);
    let p99 = percentile(&samples, 0.99);
    let budget = Duration::from_secs_f64(budget_ms() / 1000.0);
    println!(
        "wire latency: p50={:?} p99={:?} budget={:?} samples={SAMPLES}",
        p50, p99, budget
    );
    assert!(
        p99 <= budget,
        "wire p99 {:?} exceeds budget {:?}",
        p99,
        budget
    );
}
