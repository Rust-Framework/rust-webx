//! Externally visible parity between the hyper wire path and the neutral
//! transport port.
//!
//! One app profile, one request matrix, two transports:
//!
//! * hyper side — a real `Host` listening on an ephemeral TCP port, driven over
//!   the wire by plain HTTP/1.1 requests through `reqwest`;
//! * neutral side — the neutral `TransportRequest`/`TransportResponse` port
//!   driven one request at a time through `TestHost::serve_transport_request`,
//!   with no hyper types and no socket involved.
//!
//! The engine claims that a non-hyper transport reuses the complete pipeline
//! (router, middleware, binding, handler dispatch, error writer, response
//! assembly). This test pins that claim at the externally visible boundary:
//! status, content type and payload must agree for GET/POST/404/403, and HEAD
//! must agree on status, empty body and `content-length` when the neutral
//! response carries one. Handler JSON bodies are compared byte-for-byte;
//! problem documents are compared as JSON values because their extension
//! members are serialized from a `HashMap` (see the TODO on the problem branch).
//!
//! Volatile framing headers are deliberately not compared: `date`, `server`,
//! `connection` and `transfer-encoding` are owned by the protocol stack, and
//! `content-length` on body-carrying responses is derived by hyper (the neutral
//! port leaves framing to the transport).

#![allow(non_snake_case)]

use std::time::{Duration, Instant};

use serde::Serialize;
use webx::*;
use webx_engine::transport::{TransportRequest, TransportResponse, TransportResponseBody};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Routes served through both transports (defined once, registered by inventory)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, WebxRequestMeta)]
struct HelloRequest;

#[get("/parity/hello")]
impl IRequest<HelloReply> for HelloRequest {}

#[derive(Debug, Serialize)]
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
            message: "hello parity",
        })
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct EchoRequest {
    #[frombody]
    data: String,
}

#[post("/parity/echo")]
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
struct BoomRequest;

#[get("/parity/boom")]
impl IRequest<Response> for BoomRequest {}

#[derive(Default)]
struct BoomHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<BoomRequest, Response> for BoomHandler {
    async fn handle(&mut self, _req: BoomRequest) -> ActionResult<Response> {
        Err(Error::Forbidden("parity forbidden".to_string()))
    }
}

// ---------------------------------------------------------------------------
// Request matrix
// ---------------------------------------------------------------------------

/// How both transports must classify the response payload.
#[derive(Clone, Copy)]
enum Payload {
    /// A handler-produced JSON body: content type is exactly `application/json`.
    Json,
    /// An engine-written problem document: type starts with
    /// `application/problem+json`.
    Problem,
}

struct Case {
    name: &'static str,
    method: &'static str,
    path: &'static str,
    /// JSON request body, when the case carries one.
    body: Option<&'static str>,
    status: u16,
    payload: Payload,
    /// Exact expected body for success cases (quietly also proves the route ran).
    expect_body: Option<&'static str>,
}

const CASES: [Case; 5] = [
    Case {
        name: "GET /parity/hello",
        method: "GET",
        path: "/parity/hello",
        body: None,
        status: 200,
        payload: Payload::Json,
        expect_body: Some(r#"{"message":"hello parity"}"#),
    },
    Case {
        name: "POST /parity/echo",
        method: "POST",
        path: "/parity/echo",
        body: Some(r#"{"data":"echo parity"}"#),
        status: 200,
        payload: Payload::Json,
        expect_body: Some(r#""echo parity""#),
    },
    Case {
        name: "GET unknown path",
        method: "GET",
        path: "/parity/missing",
        body: None,
        status: 404,
        payload: Payload::Problem,
        expect_body: None,
    },
    Case {
        name: "GET /parity/boom",
        method: "GET",
        path: "/parity/boom",
        body: None,
        status: 403,
        payload: Payload::Problem,
        expect_body: None,
    },
    Case {
        name: "HEAD /parity/hello",
        method: "HEAD",
        path: "/parity/hello",
        body: None,
        status: 200,
        payload: Payload::Json,
        expect_body: None,
    },
];

// ---------------------------------------------------------------------------
// Hyper side: a real TCP listener driven over the wire by reqwest
// ---------------------------------------------------------------------------

fn app_builder() -> HostBuilder {
    Host::builder().mode(AppMode::Development).no_spa()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

struct HyperServer {
    base_url: String,
    handle: ServerHandle,
    task: tokio::task::JoinHandle<()>,
}

impl HyperServer {
    /// Graceful shutdown, then join the listener task (bounded, no orphan).
    async fn stop(self) {
        self.handle.shutdown();
        tokio::time::timeout(REQUEST_TIMEOUT, self.task)
            .await
            .expect("hyper test server stopped within the test timeout")
            .expect("hyper test server task did not panic");
    }
}

async fn start_hyper_server() -> HyperServer {
    // Port 0 via the helper: no fixed port, retried until this host listens.
    // The bind happens inside `run_at`, so a successful connect proves the
    // listener is up; the poll yields to the host task (no fixed sleep) and is
    // bounded by a 10s deadline.
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = app_builder().build();
    let handle = host.server_handle();
    let listen_addr = addr.clone();
    let task = tokio::spawn(async move {
        let _ = host.run_at(&listen_addr).await;
    });

    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(stream) => {
                drop(stream);
                break;
            }
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "hyper test server did not listen on {addr} within {REQUEST_TIMEOUT:?}: {error}"
                );
                tokio::task::yield_now().await;
            }
        }
    }

    HyperServer {
        base_url: format!("http://{addr}"),
        handle,
        task,
    }
}

/// One response as observed through one transport.
struct Captured {
    status: u16,
    content_type: Option<String>,
    content_length: Option<String>,
    body: Vec<u8>,
}

async fn hyper_request(client: &reqwest::Client, base_url: &str, case: &Case) -> Captured {
    let method = reqwest::Method::from_bytes(case.method.as_bytes()).expect("valid HTTP method");
    let mut request = client.request(method, format!("{base_url}{}", case.path));
    if let Some(body) = case.body {
        request = request
            .header("content-type", "application/json")
            .body(body);
    }
    let response = request
        .send()
        .await
        .unwrap_or_else(|error| panic!("hyper request for `{}` failed: {error}", case.name));
    let status = response.status().as_u16();
    let content_type = header_value(response.headers(), "content-type");
    let content_length = header_value(response.headers(), "content-length");
    let body = response
        .bytes()
        .await
        .expect("read hyper response body")
        .to_vec();
    Captured {
        status,
        content_type,
        content_length,
        body,
    }
}

fn header_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn json_body(captured: &Captured, name: &str) -> serde_json::Value {
    serde_json::from_slice(&captured.body)
        .unwrap_or_else(|error| panic!("{name}: response body is not JSON: {error}"))
}

// ---------------------------------------------------------------------------
// Neutral request construction
// ---------------------------------------------------------------------------

/// Build the neutral request for a matrix case: same method, path, headers and
/// body the hyper side sends, with a loopback peer and no hyper types.
fn transport_request(case: &Case) -> TransportRequest {
    let mut builder = http::Request::builder()
        .method(case.method)
        .uri(case.path)
        .header("host", "parity.test");
    if case.body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let (parts, ()) = builder
        .body(())
        .expect("build neutral transport request")
        .into_parts();
    TransportRequest::new(
        parts,
        case.body.unwrap_or_default().as_bytes().to_vec(),
        Some("127.0.0.1:40000".parse().expect("neutral peer address")),
    )
}

async fn capture(response: TransportResponse) -> Captured {
    let status = response.status;
    let content_type = response.header("content-type").map(str::to_owned);
    let content_length = response.header("content-length").map(str::to_owned);
    let body = match response.body {
        TransportResponseBody::None => Vec::new(),
        TransportResponseBody::Bytes(bytes) => bytes.to_vec(),
        TransportResponseBody::Stream(mut stream) => {
            use futures_util::StreamExt;
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                body.extend_from_slice(&chunk.expect("neutral response stream chunk"));
            }
            body
        }
        _ => unreachable!("unhandled future TransportResponseBody variant"),
    };
    Captured {
        status,
        content_type,
        content_length,
        body,
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hyper_wire_and_neutral_transport_are_externally_identical() {
    // --- hyper path: real listener, real HTTP/1.1 bytes -------------------
    let hyper_server = start_hyper_server().await;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client");
    let mut hyper_responses = Vec::with_capacity(CASES.len());
    for case in &CASES {
        hyper_responses.push(hyper_request(&client, &hyper_server.base_url, case).await);
    }
    drop(client);
    hyper_server.stop().await;

    // --- neutral path: same pipeline, no hyper -----------------------------
    let host = TestHost::from_builder(app_builder());
    let mut neutral_responses = Vec::with_capacity(CASES.len());
    for case in &CASES {
        let response = host.serve_transport_request(transport_request(case)).await;
        neutral_responses.push(capture(response).await);
    }

    // --- per-case comparison ----------------------------------------------
    assert_eq!(
        hyper_responses.len(),
        neutral_responses.len(),
        "one neutral response per hyper response"
    );

    for (index, case) in CASES.iter().enumerate() {
        let hyper = &hyper_responses[index];
        let neutral = &neutral_responses[index];
        let name = case.name;

        assert_eq!(hyper.status, case.status, "{name}: hyper status");
        assert_eq!(neutral.status, case.status, "{name}: neutral status");

        // HEAD: status, empty body and (when the neutral response carries one)
        // the same representation `content-length`; HEAD responses legitimately
        // drop the body on both paths.
        if case.method == "HEAD" {
            assert!(
                hyper.body.is_empty(),
                "{name}: hyper HEAD body must be empty, got {:?}",
                String::from_utf8_lossy(&hyper.body)
            );
            assert!(
                neutral.body.is_empty(),
                "{name}: neutral HEAD body must be empty, got {:?}",
                String::from_utf8_lossy(&neutral.body)
            );
            if let Some(neutral_length) = neutral.content_length.as_deref() {
                assert_eq!(
                    hyper.content_length.as_deref(),
                    Some(neutral_length),
                    "{name}: HEAD content-length parity"
                );
            }
            continue;
        }

        match case.payload {
            Payload::Json => {
                assert_eq!(
                    hyper.content_type.as_deref(),
                    Some("application/json"),
                    "{name}: hyper JSON content type"
                );
                assert_eq!(
                    neutral.content_type.as_deref(),
                    Some("application/json"),
                    "{name}: neutral JSON content type"
                );
                if let Some(expected) = case.expect_body {
                    assert_eq!(
                        hyper.body,
                        expected.as_bytes(),
                        "{name}: hyper body is the handler payload"
                    );
                    assert_eq!(
                        neutral.body,
                        expected.as_bytes(),
                        "{name}: neutral body is the handler payload"
                    );
                }
                // Handler JSON is emitted by one serializer with a fixed field
                // order, so these bytes are stable and must match exactly.
                assert_eq!(
                    hyper.body, neutral.body,
                    "{name}: JSON body bytes must match the hyper path exactly"
                );
            }
            Payload::Problem => {
                let hyper_type = hyper
                    .content_type
                    .as_deref()
                    .expect("hyper problem response has a content type");
                assert!(
                    hyper_type.starts_with("application/problem+json"),
                    "{name}: hyper content type {hyper_type:?} is not a problem document"
                );
                assert_eq!(
                    neutral.content_type.as_deref(),
                    Some(hyper_type),
                    "{name}: problem content-type parity"
                );

                // TODO(transport-parity): problem documents are compared as JSON
                // values rather than raw bytes. `ProblemDetails::extensions` is a
                // `HashMap` serialized with `#[serde(flatten)]`
                // (crates/core/src/problem.rs), so every independently built
                // problem response emits its extension members (`code`,
                // `retryable`, `traceId`) in a random order. That is a
                // pre-existing engine serialization detail, not a transport
                // divergence: the same request over hyper twice can differ
                // byte-wise the same way. Tighten this to a byte comparison once
                // the engine emits a stable extension order.
                let hyper_problem = json_body(hyper, name);
                let neutral_problem = json_body(neutral, name);
                assert_eq!(
                    hyper_problem, neutral_problem,
                    "{name}: problem JSON value must match the hyper path"
                );
                assert_eq!(
                    neutral_problem["status"],
                    serde_json::Value::from(case.status),
                    "{name}: problem status member"
                );
            }
        }
    }
}
