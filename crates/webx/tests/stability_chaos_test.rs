//! Chaos tests: overload shedding, graceful shutdown under load, and a silent
//! upstream. Ignored by default; `stability.yml` runs them nightly.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
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

#[derive(Default, WebxRequestMeta)]
struct PanicRequest;

#[get("/panic")]
impl IRequest<EchoView> for PanicRequest {}

#[derive(Default)]
struct PanicHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PanicRequest, EchoView> for PanicHandler {
    async fn handle(&mut self, _req: PanicRequest) -> ActionResult<EchoView> {
        panic!("chaos: induced handler panic");
    }
}

#[derive(Default, WebxRequestMeta)]
struct SmallBodyRequest {
    #[frombody]
    data: String,
}

#[post("/small")]
#[max_body_size(16)]
impl IRequest<EchoView> for SmallBodyRequest {}

#[derive(Default)]
struct SmallBodyHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<SmallBodyRequest, EchoView> for SmallBodyHandler {
    async fn handle(&mut self, req: SmallBodyRequest) -> ActionResult<EchoView> {
        // The body is bound (and size-capped); echoing its length keeps the
        // bound field meaningful without changing the response shape.
        let _ = req.data.len();
        Ok(EchoView { ok: true })
    }
}

async fn read_response(stream: &mut TcpStream) -> std::io::Result<u16> {
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
    Ok(status)
}

async fn request(stream: &mut TcpStream, path: &str) -> std::io::Result<u16> {
    let request = format!("GET {path} HTTP/1.1\r\nHost: chaos\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    read_response(stream).await
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("chaos server did not become ready");
}

/// A connection storm beyond `MaxConnections` must be shed with `503`, not
/// queued forever — and the server must recover once the storm ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos: nightly CI"]
async fn chaos_connection_storm_is_shed_with_503() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.useOptions(|options| {
                options.app.max_connections = 4;
                options.server.admission_timeout_ms = 50;
            })
        })
        .build();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    // Open the storm and keep every socket alive while reading responses: the
    // admitted keep-alive connections hold their slots, so the rest are shed.
    let mut sockets = Vec::new();
    for _ in 0..24 {
        sockets.push(
            TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("storm connection"),
        );
    }
    for socket in sockets.iter_mut() {
        socket
            .write_all(b"GET /echo HTTP/1.1\r\nHost: storm\r\n\r\n")
            .await
            .expect("storm write");
    }
    let mut statuses = Vec::new();
    for socket in sockets.iter_mut() {
        match tokio::time::timeout(Duration::from_secs(5), read_response(socket)).await {
            Ok(Ok(status)) => statuses.push(status),
            _ => statuses.push(0),
        }
    }
    drop(sockets);
    assert!(
        statuses.contains(&503),
        "expected at least one shed connection, got: {statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .all(|status| matches!(*status, 0 | 200 | 503)),
        "unexpected statuses under storm: {statuses:?}"
    );

    // Recovery: after the storm the server serves normally again.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("recovery connection");
    assert_eq!(request(&mut stream, "/echo").await.expect("recovery"), 200);
}

/// Shutdown while requests are in flight: in-flight requests finish (no 5xx),
/// the port stops accepting, and the process drains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos: nightly CI"]
async fn chaos_graceful_shutdown_under_load() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let load = tokio::spawn(async move {
        let (mut ok, mut server_errors) = (0u64, 0u64);
        let mut stream = match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => stream,
            Err(_) => return (ok, server_errors),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match request(&mut stream, "/echo").await {
                Ok(200) => ok += 1,
                Ok(status) if status >= 500 => server_errors += 1,
                Ok(_) => {}
                // The drain closes keep-alive connections; that is expected.
                Err(_) => break,
            }
        }
        (ok, server_errors)
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown();
    let (ok, server_errors) = load.await.expect("load task");
    assert!(ok > 10, "load should have completed requests, got {ok}");
    assert_eq!(server_errors, 0, "graceful shutdown produced 5xx responses");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server kept accepting connections after shutdown");
}

/// A silent upstream must produce a typed timeout, not hang the caller.
#[tokio::test]
#[ignore = "chaos: nightly CI"]
async fn chaos_silent_upstream_times_out() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("silent upstream");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    // Accept, then never respond.
                    tokio::spawn(async move {
                        let _keep = stream;
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    });
                }
                Err(_) => return,
            }
        }
    });

    let mut options = HttpClientOptions::new(format!("http://{address}"));
    options.connect_timeout = Duration::from_millis(500);
    options.request_timeout = Some(Duration::from_millis(100));
    options.read_timeout = Some(Duration::from_millis(100));
    let client = HttpClient::new("silent", options).expect("client");

    let started = Instant::now();
    let result = client.execute(OutboundRequest::get("/slow")).await;
    assert!(result.is_err(), "a silent upstream must not succeed");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timeout took too long: {:?}",
        started.elapsed()
    );
}
async fn post_request(stream: &mut TcpStream, path: &str, body: &[u8]) -> std::io::Result<u16> {
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: chaos\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    read_response(stream).await
}

/// A panicking handler is contained to its request: the client gets a typed
/// `500`, sibling connections are unaffected, and the worker keeps serving.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos: nightly CI"]
async fn chaos_handler_panic_is_contained() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let mut tasks = Vec::new();
    for _ in 0..15 {
        tasks.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("panic connection");
            request(&mut stream, "/panic").await.unwrap_or(0)
        }));
    }
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.expect("panic task"));
    }
    assert!(
        statuses.iter().all(|status| *status == 500),
        "every panicking request must be a typed 500, got: {statuses:?}"
    );

    // The worker survived: a normal request on a fresh connection still works.
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("recovery connection");
    assert_eq!(request(&mut stream, "/echo").await.expect("recovery"), 200);
}

/// An oversized body is rejected with `413` before the handler runs, and the
/// keep-alive connection remains usable for the next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos: nightly CI"]
async fn chaos_oversized_body_is_shed_and_connection_survives() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("limit connection");
    let oversized = br#"{"data":"this body is far beyond the sixteen byte route limit"}"#;
    let status = post_request(&mut stream, "/small", oversized)
        .await
        .expect("oversized request");
    assert_eq!(status, 413, "oversized body must be rejected with 413");

    // Same keep-alive connection: the next request is served normally.
    assert_eq!(
        request(&mut stream, "/echo")
            .await
            .expect("post-413 request"),
        200,
        "the connection must stay usable after a 413"
    );
}
