//! Transport neutral request port drives the real pipeline.
//!
//! A non-hyper transport supplies neutral `TransportRequest` values and records
//! `TransportResponse` values. Router, middleware, handler dispatch, JSON
//! binding, the error writer, panic boundary, response assembly and the SSE
//! lifecycle are the production ones.

#![allow(non_snake_case)]

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use webx::*;
use webx_engine::transport::{
    IHttpTransport, TransportBodyError, TransportBodyFrame, TransportRequest, TransportRequestBody,
    TransportRequestBodyStream, TransportResponse, TransportResponseBody, TransportRuntime,
};

// ---------------------------------------------------------------------------
// Routes served through the neutral port
// ---------------------------------------------------------------------------

#[derive(Debug, Default, WebxRequestMeta)]
struct HelloRequest;

#[get("/transport/hello")]
impl IRequest<HelloReply> for HelloRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct HelloReply {
    message: &'static str,
}

#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<HelloRequest, HelloReply> for HelloHandler {
    async fn handle(&mut self, _req: HelloRequest) -> ActionResult<HelloReply> {
        Ok(HelloReply {
            message: "hello transport",
        })
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct EchoRequest {
    #[frombody]
    data: String,
}

#[post("/transport/echo")]
impl IRequest<String> for EchoRequest {}

#[derive(Default)]
struct EchoHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<EchoRequest, String> for EchoHandler {
    async fn handle(&mut self, req: EchoRequest) -> ActionResult<String> {
        Ok(req.data)
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct CertRequest {
    #[fromheader("x-webx-client-cert-sha256")]
    cert: Option<String>,
}

#[get("/transport/cert")]
impl IRequest<String> for CertRequest {}

#[derive(Default)]
struct CertHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CertRequest, String> for CertHandler {
    async fn handle(&mut self, req: CertRequest) -> ActionResult<String> {
        Ok(req.cert.unwrap_or_default())
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct BoomRequest;

#[get("/transport/boom")]
impl IRequest<Response> for BoomRequest {}

#[derive(Default)]
struct BoomHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<BoomRequest, Response> for BoomHandler {
    async fn handle(&mut self, _req: BoomRequest) -> ActionResult<Response> {
        Err(Error::forbidden("no access".to_string()))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct StreamRequest;

#[get("/transport/sse")]
impl IRequest<Response> for StreamRequest {}

#[derive(Default)]
struct StreamHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<StreamRequest, Response> for StreamHandler {
    async fn handle(&mut self, _req: StreamRequest) -> ActionResult<Response> {
        let events = futures_util::stream::iter([Ok(SseEvent::data("hello"))]);
        Ok(Box::new(SseResult::new(events)))
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct PanicRequest;

#[get("/transport/panic")]
impl IRequest<Response> for PanicRequest {}

#[derive(Default)]
struct PanicHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PanicRequest, Response> for PanicHandler {
    async fn handle(&mut self, _req: PanicRequest) -> ActionResult<Response> {
        panic!("handler panicked on purpose: the neutral transport must contain it");
    }
}

// ---------------------------------------------------------------------------
// Request builder + response capture
// ---------------------------------------------------------------------------

fn request(method: &str, path: &str, body: &str) -> TransportRequest {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "transport.test");
    if !body.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder.body(()).expect("build transport request");
    let (parts, ()) = request.into_parts();
    TransportRequest::new(
        parts,
        body.as_bytes().to_vec(),
        Some("127.0.0.1:40000".parse().expect("peer address")),
    )
}

type ResponseSink = Arc<Mutex<Vec<TransportResponse>>>;

/// A listener backend that owns a fixed set of decoded requests.
struct FakeListener {
    requests: Mutex<Vec<TransportRequest>>,
    responses: ResponseSink,
}

#[async_trait::async_trait]
impl IHttpTransport for FakeListener {
    fn kind(&self) -> &'static str {
        "in-process"
    }

    async fn run(self: Arc<Self>, runtime: TransportRuntime) -> Result<()> {
        let requests = std::mem::take(
            &mut *self
                .requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        let server = runtime.server();
        for request in requests {
            let response = server.serve_transport_request(request).await;
            self.responses
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(response);
        }
        Ok(())
    }
}

/// One fully read response (the transport would own framing on the wire).
struct Captured {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("response body is JSON")
    }
}

async fn capture(response: TransportResponse) -> Captured {
    let status = response.status;
    let headers = response.headers;
    let body = match response.body {
        TransportResponseBody::None => Vec::new(),
        TransportResponseBody::Bytes(bytes) => bytes.to_vec(),
        TransportResponseBody::Stream(mut stream) => {
            use futures_util::StreamExt;
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                body.extend_from_slice(&chunk.expect("stream response chunk"));
            }
            body
        }
        _ => unreachable!("unhandled future TransportResponseBody variant"),
    };
    Captured {
        status,
        headers,
        body,
    }
}

async fn drain(responses: ResponseSink) -> Vec<Captured> {
    let pending = std::mem::take(
        &mut *responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    let mut captured = Vec::with_capacity(pending.len());
    for response in pending {
        captured.push(capture(response).await);
    }
    captured
}

fn test_host() -> TestHost {
    TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn neutral_transport_reuses_the_full_pipeline() {
    let host = test_host();
    let captured = [
        capture(
            host.serve_transport_request(request("GET", "/transport/hello", ""))
                .await,
        )
        .await,
        capture(
            host.serve_transport_request(request("POST", "/transport/echo", "{\"data\":\"echo\"}"))
                .await,
        )
        .await,
        capture(
            host.serve_transport_request(request("GET", "/transport/missing", ""))
                .await,
        )
        .await,
        capture(
            host.serve_transport_request(request("GET", "/transport/boom", ""))
                .await,
        )
        .await,
        capture(
            host.serve_transport_request(request("GET", "/transport/sse", ""))
                .await,
        )
        .await,
    ];

    // 1. GET /transport/hello -> handler JSON
    assert_eq!(captured[0].status, 200);
    assert_eq!(captured[0].header("content-type"), Some("application/json"));
    assert_eq!(captured[0].json()["message"], "hello transport");

    // 2. POST JSON body -> binding + handler dispatch
    assert_eq!(captured[1].status, 200);
    assert_eq!(
        serde_json::from_slice::<String>(&captured[1].body).expect("echo JSON"),
        "echo"
    );

    // 3. Unknown route -> the engine's 404 problem+json writer
    assert_eq!(captured[2].status, 404);
    assert!(captured[2]
        .header("content-type")
        .expect("content type")
        .starts_with("application/problem+json"));
    let missing = captured[2].json();
    assert_eq!(missing["status"], 404);
    assert_eq!(missing["code"], "not_found");

    // 4. Handler error -> the engine's error mapping (403 + problem code)
    assert_eq!(captured[3].status, 403);
    let forbidden = captured[3].json();
    assert_eq!(forbidden["status"], 403);
    assert_eq!(forbidden["code"], "forbidden");

    // 5. SSE -> streamed neutral body, with the existing lifecycle events.
    assert_eq!(captured[4].status, 200);
    assert!(captured[4]
        .header("content-type")
        .expect("content type")
        .starts_with("text/event-stream"));
    let sse = String::from_utf8_lossy(&captured[4].body);
    assert!(sse.contains("data:hello"), "missing SSE frame: {sse:?}");
}

/// A handler panic is contained exactly like on the hyper path: the request is
/// answered with the engine's `500` problem document and the next request keeps
/// working.
#[tokio::test]
async fn a_panicking_handler_is_contained_and_the_pipeline_keeps_serving() {
    let host = test_host();

    let panicked = capture(
        host.serve_transport_request(request("GET", "/transport/panic", ""))
            .await,
    )
    .await;
    assert_eq!(panicked.status, 500);
    assert!(
        panicked
            .header("content-type")
            .expect("content type")
            .starts_with("application/problem+json"),
        "a panic must use the engine's problem writer"
    );
    assert_eq!(panicked.json()["status"], 500);

    let next = capture(
        host.serve_transport_request(request("GET", "/transport/hello", ""))
            .await,
    )
    .await;
    assert_eq!(next.status, 200);
    assert_eq!(next.json()["message"], "hello transport");
}

#[tokio::test]
async fn listener_transport_serves_requests_through_the_runtime() {
    let host = test_host();
    let responses: ResponseSink = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::new(FakeListener {
        requests: Mutex::new(vec![
            request("GET", "/transport/hello", ""),
            request("GET", "/transport/hello", ""),
        ]),
        responses: Arc::clone(&responses),
    });

    host.run_http_transport(listener as Arc<dyn IHttpTransport>)
        .await
        .expect("run in-process listener transport");

    let captured = drain(responses).await;
    assert_eq!(captured.len(), 2);
    for response in &captured {
        assert_eq!(response.status, 200);
        assert_eq!(response.json()["message"], "hello transport");
    }
}

#[tokio::test]
async fn neutral_transport_streams_a_multi_frame_request_body() {
    let host = test_host();

    let request = http::Request::builder()
        .method("POST")
        .uri("/transport/echo")
        .header("host", "transport.test")
        .header("content-type", "application/json")
        .body(())
        .expect("build streaming request");
    let (parts, ()) = request.into_parts();
    let frames: Vec<std::result::Result<TransportBodyFrame, TransportBodyError>> = vec![
        Ok(TransportBodyFrame::Data(bytes::Bytes::from_static(
            b"{\"data\":\"multi",
        ))),
        Ok(TransportBodyFrame::Data(bytes::Bytes::from_static(b"\"}"))),
    ];
    let body = TransportRequestBody::streaming(
        Box::pin(futures_util::stream::iter(frames)) as TransportRequestBodyStream,
        None,
    );
    let request = TransportRequest::with_body(
        parts,
        body,
        Some("127.0.0.1:40000".parse().expect("peer address")),
    );

    let captured = capture(host.serve_transport_request(request).await).await;
    assert_eq!(captured.status, 200);
    assert_eq!(
        serde_json::from_slice::<String>(&captured.body).expect("echo JSON"),
        "multi"
    );
}

#[tokio::test]
async fn client_cert_header_is_transport_owned() {
    let host = test_host();

    // A spoofed header on the neutral request must be stripped.
    let mut spoofed = request("GET", "/transport/cert", "");
    spoofed.parts.headers.insert(
        "x-webx-client-cert-sha256",
        "spoofed-by-client".parse().expect("header value"),
    );
    // The verified fingerprint supplied by the TLS terminator must be injected.
    let verified =
        request("GET", "/transport/cert", "").with_client_cert_fingerprint("verified-sha256");

    let spoofed = capture(host.serve_transport_request(spoofed).await).await;
    let verified = capture(host.serve_transport_request(verified).await).await;

    assert_eq!(
        serde_json::from_slice::<String>(&spoofed.body).expect("spoofed JSON"),
        "",
        "client-supplied fingerprint header must be removed"
    );
    assert_eq!(
        serde_json::from_slice::<String>(&verified.body).expect("verified JSON"),
        "verified-sha256",
        "transport-supplied fingerprint must be injected"
    );
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

#[tokio::test]
async fn host_injects_alt_svc_when_configured() {
    let host = TestHost::from_builder(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .use_alt_svc("h3=\":4433\"; ma=86400"),
    );

    let captured = capture(
        host.serve_transport_request(request("GET", "/transport/hello", ""))
            .await,
    )
    .await;
    assert_eq!(
        captured.header("alt-svc"),
        Some("h3=\":4433\"; ma=86400"),
        "use_alt_svc must advertise the configured UDP endpoint"
    );
}

#[tokio::test]
async fn host_starts_listener_transport_backends() {
    let responses: ResponseSink = Arc::new(Mutex::new(Vec::new()));
    let listener = Arc::new(FakeListener {
        requests: Mutex::new(vec![request("GET", "/transport/hello", "")]),
        responses: Arc::clone(&responses),
    });
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_http_transport(listener as Arc<dyn IHttpTransport>)
        .build();
    let handle = host.server_handle();
    let port = free_port();
    let task = tokio::spawn(async move { host.run_at(&format!("127.0.0.1:{port}")).await });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let ready = !responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty();
        if ready {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "host-driven transport backend did not receive a request"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    handle.shutdown();
    tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .expect("host stops after shutdown")
        .expect("host task joins")
        .expect("host run succeeds");

    let captured = drain(responses).await;
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].status, 200);
    assert_eq!(captured[0].json()["message"], "hello transport");
}
