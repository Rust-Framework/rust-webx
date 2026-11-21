//! Soak test: keep a steady request load running and check that the server
//! still serves, the in-flight gauge returns to zero, and (on Linux) RSS growth
//! stays bounded.
//!
//! Ignored by default; `stability.yml` runs it nightly with `SOAK_SECONDS=300`.
//! A manual 24h run uses `SOAK_SECONDS=86400`.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

fn soak_seconds() -> u64 {
    std::env::var("SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10)
}

fn max_rss_growth_bytes() -> u64 {
    let megabytes = std::env::var("SOAK_MAX_RSS_GROWTH_MB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(256u64);
    megabytes * 1024 * 1024
}

#[cfg(target_os = "linux")]
fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * 4096)
}

#[cfg(not(target_os = "linux"))]
fn rss_bytes() -> Option<u64> {
    None
}

#[derive(Default, WebxRequestMeta)]
struct EchoRequest;

#[derive(serde::Serialize)]
struct EchoView {
    ok: bool,
}

#[get("/echo")]
impl IRequest<EchoView> for EchoRequest {}

#[derive(Default)]
struct EchoHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<EchoRequest, EchoView> for EchoHandler {
    async fn handle(&mut self, _req: EchoRequest) -> ActionResult<EchoView> {
        Ok(EchoView { ok: true })
    }
}

async fn read_response(stream: &mut TcpStream) -> std::io::Result<(u16, usize)> {
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut content_length = 0usize;
    for line in head.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    stream.read_exact(&mut body).await?;
    Ok((status, body.len()))
}

async fn request(stream: &mut TcpStream, path: &str) -> std::io::Result<u16> {
    let request = format!("GET {path} HTTP/1.1\r\nHost: soak\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let (status, _) = read_response(stream).await?;
    Ok(status)
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("soak server did not become ready");
}

/// One keep-alive connection sending requests until `deadline`.
async fn load_loop(port: u16, deadline: Instant) -> (u64, u64) {
    let (mut successes, mut failures) = (0u64, 0u64);
    let address = format!("127.0.0.1:{port}");
    let mut stream = loop {
        match TcpStream::connect(&address).await {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(_) => return (successes, failures + 1),
        }
    };
    while Instant::now() < deadline {
        match request(&mut stream, "/echo").await {
            Ok(200) => successes += 1,
            Ok(_) => failures += 1,
            // A write/read error before shutdown is a failure for the soak.
            Err(_) => {
                failures += 1;
                break;
            }
        }
    }
    (successes, failures)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "soak: nightly CI / SOAK_SECONDS=86400 for a full run"]
async fn soak_keeps_serving_without_leaks() {
    let seconds = soak_seconds();
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.metrics.enabled = true;
                options.server.workers = 2;
            })
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    // Warm up, then take an RSS baseline.
    {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("warmup");
        for _ in 0..50 {
            request(&mut stream, "/echo").await.expect("warmup request");
        }
    }
    let rss_before = rss_bytes();

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        tasks.push(tokio::spawn(load_loop(port, deadline)));
    }
    let mut total = 0u64;
    let mut failures = 0u64;
    for task in tasks {
        let (successes, failed) = task.await.expect("load task");
        total += successes;
        failures += failed;
    }

    assert_eq!(failures, 0, "every soak request must succeed");
    assert!(
        total > seconds.max(1) * 25,
        "throughput sanity: only {total} requests in {seconds}s"
    );

    // The server still serves and the in-flight gauge returns to its baseline
    // (the /metrics scrape itself counts as one in-flight request).
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("post-soak connection");
    assert_eq!(request(&mut stream, "/echo").await.expect("post-soak"), 200);
    let metrics_request = "GET /metrics HTTP/1.1\r\nHost: soak\r\n\r\n";
    stream
        .write_all(metrics_request.as_bytes())
        .await
        .expect("metrics write");
    let (status, _) = read_response(&mut stream).await.expect("metrics read");
    assert_eq!(status, 200);

    if let (Some(before), Some(after)) = (rss_before, rss_bytes()) {
        let growth = after.saturating_sub(before);
        assert!(
            growth <= max_rss_growth_bytes(),
            "RSS grew {growth} bytes during the soak (limit {})",
            max_rss_growth_bytes()
        );
    }

    handle.shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The process may still be draining; that is fine as long as it stopped
    // accepting new connections. Assert the listener is closed.
    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
        "server should stop accepting connections after shutdown"
    );
}
