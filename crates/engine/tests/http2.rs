//! h2c prior-knowledge integration: the engine's `auto` connection path must
//! serve HTTP/2 on a shared worker without a TLS listener.

use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpStream;
use webx_core::http::IHttpContext;
use webx_core::routing::HttpMethod;
use webx_engine::{
    BodyLimits, EngineConfig, Listener, Pipeline, RouteTarget, Router, Server, Shared,
};

struct OkTarget;

#[async_trait::async_trait]
impl RouteTarget for OkTarget {
    async fn handle(&self, ctx: &mut webx_engine::EngineContext) -> webx_core::error::Result<()> {
        ctx.response_mut().set_status(200);
        ctx.response_mut().write_bytes(b"h2-ok".to_vec()).await
    }
}

fn shared() -> Arc<Shared> {
    let provider = rust_dix::ServiceCollection::new()
        .build()
        .expect("provider");
    let mut router = Router::new();
    router.insert(HttpMethod::Get, "/", Arc::new(OkTarget));
    let (shutdown, _) = tokio::sync::watch::channel(false);
    Arc::new(Shared {
        config: Arc::new(EngineConfig {
            workers: 1,
            max_connections: 8,
            first_request_timeout: Duration::from_secs(5),
            ..EngineConfig::default()
        }),
        router: Arc::new(router),
        pipeline: Arc::new(Pipeline::new(Vec::new())),
        dispatch: Arc::new(webx_core::DispatchRuntime::new(
            provider,
            Arc::new(webx_core::route::scan::HandlerCache::build()),
        )),
        body_limits: Arc::new(BodyLimits::default()),
        error_writers: Arc::new(Vec::new()),
        tls: None,
        shutdown,
        shutdown_token: webx_core::hosted::CancellationToken::new(),
        immediate_shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        immediate_watch: tokio::sync::watch::channel(false).0,
        started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http2_prior_knowledge_serves_requests() {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = Server::new(shared());
    let handle = server.handle();
    let task = tokio::spawn(server.run_listeners(vec![Listener::plain(listener)]));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let tcp = TcpStream::connect(addr).await.expect("tcp");
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
            .await
            .expect("h2 handshake");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    for _ in 0..2 {
        let request = hyper::Request::builder()
            .uri("http://localhost/")
            .body(Empty::<Bytes>::new())
            .expect("request");
        let response = sender.send_request(request).await.expect("h2 response");
        assert_eq!(response.status().as_u16(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(&body[..], b"h2-ok");
    }

    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("h2 server shuts down")
        .expect("task")
        .expect("server");
}
