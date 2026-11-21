//! Wire-level WebSocket upgrade test (2.3-1).
//!
//! Proves the engine-side contract: an upgrade request reaches the route, the
//! endpoint installs an [`IUpgradeSession`] and writes `101`, and the session
//! receives the **raw upgraded byte stream** (WS framing is the framework
//! layer's job, tested separately in the webx crate).

use std::net::TcpListener as StdTcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use rust_dix::ServiceCollection;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx_core::dispatch_runtime::DispatchRuntime;
use webx_core::error::{Error, IErrorResponseWriter, Result};
use webx_core::hosted::CancellationToken;
use webx_core::http::IHttpContext;
use webx_core::realtime::{
    websocket_meta, ConnectionMeta, IUpgradeSession, TransportKind, UpgradedTransport,
};
use webx_core::route::scan::HandlerCache;
use webx_core::routing::HttpMethod;
use webx_engine::{
    BodyLimits, EngineConfig, EngineContext, Pipeline, RouteTarget, Router, Server, Shared,
};

struct UpgradeTarget;

#[async_trait::async_trait]
impl RouteTarget for UpgradeTarget {
    async fn handle(&self, ctx: &mut EngineContext) -> Result<()> {
        ctx.response_mut().set_status(101);
        ctx.response_mut().set_header("connection", "Upgrade");
        ctx.response_mut().set_header("upgrade", "websocket");
        ctx.response_mut()
            .set_header("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        ctx.set_upgrade_session(Arc::new(EchoSession), websocket_meta(None));
        Ok(())
    }
}

struct EchoSession;

#[async_trait::async_trait]
impl IUpgradeSession for EchoSession {
    async fn run(&self, mut transport: UpgradedTransport, meta: ConnectionMeta) -> Result<()> {
        assert_eq!(meta.transport, TransportKind::WebSocket);
        let mut buffer = vec![0u8; 4096];
        loop {
            match transport.read(&mut buffer).await {
                Ok(0) => return Ok(()),
                Ok(read) => {
                    transport
                        .write_all(&buffer[..read])
                        .await
                        .map_err(|error| {
                            Error::Internal(format!("upgraded transport write failed: {error}"))
                        })?;
                    transport.flush().await.map_err(|error| {
                        Error::Internal(format!("upgraded transport flush failed: {error}"))
                    })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(error) => {
                    return Err(Error::Internal(format!(
                        "upgraded transport read failed: {error}"
                    )))
                }
            }
        }
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        workers: 1,
        max_connections: 8,
        first_request_timeout: Duration::from_secs(5),
        ..EngineConfig::default()
    }
}

fn shared() -> Arc<Shared> {
    let provider = ServiceCollection::new().build().expect("provider");
    let dispatch = Arc::new(DispatchRuntime::new(
        provider,
        Arc::new(HandlerCache::build()),
    ));
    let mut router = Router::new();
    router.insert(HttpMethod::Get, "/ws", Arc::new(UpgradeTarget));
    let (shutdown, _) = tokio::sync::watch::channel(false);
    Arc::new(Shared {
        config: Arc::new(config()),
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
        admission: Arc::new(webx_engine::shared::AdmissionStats::default()),
    })
}

async fn wait_ready(addr: std::net::SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("engine did not start at {addr}");
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await.expect("read header");
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return String::from_utf8_lossy(&head).into_owned();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_upgrade_hands_raw_transport_to_the_session() {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = Server::new(shared());
    let handle = server.handle();
    let task = tokio::spawn(server.run(listener));
    wait_ready(addr).await;

    let mut client = TcpStream::connect(addr).await.expect("client");
    let request = "GET /ws HTTP/1.1\r\n\
                   Host: test\r\n\
                   Connection: Upgrade\r\n\
                   Upgrade: websocket\r\n\
                   Sec-WebSocket-Version: 13\r\n\
                   Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                   \r\n";
    client.write_all(request.as_bytes()).await.expect("write");

    let head = tokio::time::timeout(Duration::from_secs(5), read_head(&mut client))
        .await
        .expect("101 response deadline");
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "expected 101, got: {head}"
    );
    assert!(
        head.to_ascii_lowercase()
            .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "accept key missing: {head}"
    );

    // The session receives the raw upgraded stream; echo bytes round-trip.
    client
        .write_all(b"hello-engine")
        .await
        .expect("write payload");
    let mut echoed = vec![0u8; b"hello-engine".len()];
    tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut echoed))
        .await
        .expect("echo deadline")
        .expect("echo read");
    assert_eq!(echoed, b"hello-engine");

    drop(client);
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.shutdown();
    task.await.expect("server task").expect("server result");
}
