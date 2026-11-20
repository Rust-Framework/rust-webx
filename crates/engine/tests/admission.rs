//! Wire-level tests for the engine's admission and listener model.
//!
//! These deliberately do **not** go through `Host`: the connection governor,
//! first-request deadline and multi-listener sharing are engine invariants and
//! must be regression-tested where they live.

use std::net::TcpListener as StdTcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use rust_dix::ServiceCollection;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx_core::dispatch_runtime::DispatchRuntime;
use webx_core::error::{IErrorResponseWriter, Result};
use webx_core::hosted::CancellationToken;
use webx_core::http::IHttpContext;
use webx_core::route::scan::HandlerCache;
use webx_core::routing::HttpMethod;
use webx_engine::{
    BodyLimits, EngineConfig, EngineContext, Listener, Pipeline, RouteTarget, Router, Server,
    Shared,
};

struct OkTarget;

#[async_trait::async_trait]
impl RouteTarget for OkTarget {
    async fn handle(&self, ctx: &mut EngineContext) -> Result<()> {
        ctx.response_mut().set_status(200);
        ctx.response_mut().write_bytes(b"ok".to_vec()).await
    }
}

fn config(first_request_timeout: Duration, max_connections: usize) -> EngineConfig {
    EngineConfig {
        workers: 1,
        max_connections,
        first_request_timeout,
        ..EngineConfig::default()
    }
}

fn shared(config: EngineConfig) -> Arc<Shared> {
    let provider = ServiceCollection::new().build().expect("provider");
    let dispatch = Arc::new(DispatchRuntime::new(
        provider,
        Arc::new(HandlerCache::build()),
    ));
    let mut router = Router::new();
    router.insert(HttpMethod::Get, "/", Arc::new(OkTarget));
    let (shutdown, _) = tokio::sync::watch::channel(false);
    Arc::new(Shared {
        config: Arc::new(config),
        router: Arc::new(router),
        pipeline: Arc::new(Pipeline::new(Vec::new())),
        dispatch,
        body_limits: Arc::new(BodyLimits::default()),
        error_writers: Arc::new(Vec::<Arc<dyn IErrorResponseWriter>>::new()),
        tls: None,
        shutdown,
        shutdown_token: CancellationToken::new(),
        immediate_shutdown: Arc::new(AtomicBool::new(false)),
        immediate_watch: tokio::sync::watch::channel(false).0,
        started: Arc::new(AtomicBool::new(false)),
    })
}

async fn get(addr: std::net::SocketAddr) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
        .await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

async fn wait_ready(addr: std::net::SocketAddr) {
    for _ in 0..100 {
        if get(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("engine did not start at {addr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_connections_do_not_exhaust_the_global_slot() {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let shared = shared(config(Duration::from_millis(200), 2));
    let server = Server::new(shared);
    let handle = server.handle();
    let task = tokio::spawn(server.run(listener));
    wait_ready(addr).await;

    // Two clients send no bytes. They occupy the two permits until the
    // first-request watchdog closes them.
    let _silent_a = TcpStream::connect(addr).await.expect("silent a");
    let _silent_b = TcpStream::connect(addr).await.expect("silent b");
    tokio::time::sleep(Duration::from_millis(600)).await;

    let response = tokio::time::timeout(Duration::from_secs(2), get(addr))
        .await
        .expect("a new client is admitted after silent sockets expire")
        .expect("GET succeeds");
    assert!(response.contains("200"), "{response}");

    handle.shutdown();
    task.await.expect("server task").expect("server result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_connections_is_shared_by_all_listeners() {
    let listener_a = StdTcpListener::bind("127.0.0.1:0").expect("bind a");
    let listener_b = StdTcpListener::bind("127.0.0.1:0").expect("bind b");
    let addr_a = listener_a.local_addr().expect("a");
    let addr_b = listener_b.local_addr().expect("b");
    let shared = shared(config(Duration::from_secs(2), 1));
    let server = Server::new(shared);
    let handle = server.handle();
    let task = tokio::spawn(server.run_listeners(vec![
        Listener::plain(listener_a),
        Listener::plain(listener_b),
    ]));
    wait_ready(addr_a).await;

    // A holds the single global permit. B's request stays in the kernel
    // backlog until A yields, proving that listeners do not get independent
    // MaxConnections budgets.
    let silent_a = TcpStream::connect(addr_a).await.expect("silent a");
    let mut request_b = TcpStream::connect(addr_b).await.expect("request b");
    request_b
        .write_all(b"GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
        .await
        .expect("write b");

    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(silent_a);

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), request_b.read_to_end(&mut response))
        .await
        .expect("B is served after A releases the global permit")
        .expect("read b");
    assert!(String::from_utf8_lossy(&response).contains("200"));

    handle.shutdown();
    task.await.expect("server task").expect("server result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_releases_a_connection_with_no_request() {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let shared = shared(config(Duration::from_secs(30), 4));
    let server = Server::new(shared);
    let handle = server.handle();
    let task = tokio::spawn(server.run(listener));
    wait_ready(addr).await;

    // Graceful shutdown must also release a connection that has not started a
    // request; the first-request watchdog and shutdown receiver make that
    // connection cancellable.
    let _silent = TcpStream::connect(addr).await.expect("silent");
    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("server run completes promptly on shutdown")
        .expect("server task")
        .expect("server result");
}
