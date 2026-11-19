//! End-to-end tests against a real `hyper` server.
//!
//! These cover what a gateway actually depends on: header/body round trips,
//! streaming responses, incremental SSE decoding, timeouts and body limits.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::{self, StreamExt};
use http::{Request, Response};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use webx_client::{
    HttpClient, HttpClientError, HttpClientOptions, HttpClientRegistry, IHttpClient,
    IHttpClientFactory, OutboundRequest,
};

type TestBody = UnsyncBoxBody<Bytes, Infallible>;

fn full(body: impl Into<Bytes>) -> TestBody {
    Full::new(body.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn frame(data: &'static [u8]) -> Result<Frame<Bytes>, Infallible> {
    Ok(Frame::data(Bytes::from_static(data)))
}

fn sse_body() -> TestBody {
    let chunks = stream::iter([frame(b"data: one\n\n"), frame(b"data: two\n\n")]);
    StreamBody::new(chunks).boxed_unsync()
}

fn slow_sse_body() -> TestBody {
    let chunks = stream::unfold(0u8, |step| async move {
        match step {
            0 => Some((frame(b"data: one\n\n"), 1)),
            1 => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                Some((frame(b"data: two\n\n"), 2))
            }
            _ => None,
        }
    });
    StreamBody::new(chunks).boxed_unsync()
}

async fn echo(req: Request<Incoming>) -> Result<Response<TestBody>, Infallible> {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let traceparent = req
        .headers()
        .get("traceparent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let received_headers = req.headers().clone();
    let body = req
        .into_body()
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .unwrap_or_default();
    let header = |name: &str| {
        received_headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let value = serde_json::json!({
        "method": method,
        "path": path,
        "request_id": request_id,
        "traceparent": traceparent,
        "connection": header("connection"),
        "keep_alive": header("keep-alive"),
        "transfer_encoding": header("transfer-encoding"),
        "proxy_authorization": header("proxy-authorization"),
        "x_keep": header("x-keep"),
        "x_drop": header("x-drop"),
        "body": String::from_utf8_lossy(&body),
    });
    let bytes = serde_json::to_vec(&value).expect("echo serializes");
    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(full(bytes))
        .expect("response builds"))
}

async fn handle(req: Request<Incoming>) -> Result<Response<TestBody>, Infallible> {
    match req.uri().path() {
        "/json" => Ok(Response::builder()
            .status(200)
            .header("x-test", "yes")
            .body(full(Bytes::from_static(b"{\"ok\":true}")))
            .expect("response builds")),
        "/echo" => echo(req).await,
        "/sse" => Ok(Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(sse_body())
            .expect("response builds")),
        "/slow-body" => Ok(Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(slow_sse_body())
            .expect("response builds")),
        "/slow" => {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(Response::builder()
                .status(200)
                .body(full(Bytes::from_static(b"late")))
                .expect("response builds"))
        }
        "/big" => Ok(Response::builder()
            .status(200)
            .body(full(vec![b'x'; 2048]))
            .expect("response builds")),
        "/status500" => Ok(Response::builder()
            .status(500)
            .body(full(Bytes::from_static(b"boom")))
            .expect("response builds")),
        _ => Ok(Response::builder()
            .status(404)
            .body(full(Bytes::from_static(b"missing")))
            .expect("response builds")),
    }
}

async fn spawn_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test port");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let _ = http1::Builder::new()
                    .serve_connection(io, service_fn(handle))
                    .await;
            });
        }
    });

    format!("http://{addr}")
}

/// Minimal HTTP `CONNECT` proxy: accepts one CONNECT request, opens the tunnel
/// target and then splices the two sockets. Enough to prove the client actually
/// tunnels through `HttpClientOptions::proxy`.
async fn spawn_connect_proxy() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy port");
    let addr = listener.local_addr().expect("proxy local addr");

    tokio::spawn(async move {
        loop {
            let Ok((mut incoming, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match incoming.read(&mut byte).await {
                        Ok(1) => head.push(byte[0]),
                        _ => return,
                    }
                    if head.len() > 8192 {
                        return;
                    }
                }
                let text = String::from_utf8_lossy(&head);
                let Some(first) = text.lines().next() else {
                    return;
                };
                let mut parts = first.split_whitespace();
                let Some(method) = parts.next() else { return };
                let Some(target) = parts.next() else { return };
                if !method.eq_ignore_ascii_case("CONNECT") {
                    let _ = incoming
                        .write_all(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
                        .await;
                    return;
                }
                let Ok(mut upstream) = TcpStream::connect(target).await else {
                    let _ = incoming
                        .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                        .await;
                    return;
                };
                if incoming
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut upstream).await;
            });
        }
    });

    format!("http://{addr}")
}

fn options(base_url: String) -> HttpClientOptions {
    HttpClientOptions {
        base_url,
        ..HttpClientOptions::default()
    }
}

#[tokio::test]
async fn gets_json_with_headers() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let response = client
        .execute(OutboundRequest::get("/json"))
        .await
        .expect("request succeeds");

    assert_eq!(response.status, 200);
    assert_eq!(response.headers["x-test"].to_str().unwrap(), "yes");
    assert_eq!(
        &response.body.collect(1024).await.expect("body collects")[..],
        b"{\"ok\":true}"
    );
}

#[tokio::test]
async fn sends_method_headers_and_json_body() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let request = OutboundRequest::post("/echo")
        .header("x-request-id", "req-42")
        .json(&serde_json::json!({ "hello": "world" }))
        .expect("json body serializes");
    let response = client.execute(request).await.expect("request succeeds");
    let body = response.body.collect(4096).await.expect("body collects");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("echo is JSON");

    assert_eq!(value["method"], "POST");
    assert_eq!(value["path"], "/echo");
    assert_eq!(value["request_id"], "req-42");
    assert_eq!(value["body"], "{\"hello\":\"world\"}");
}

#[tokio::test]
async fn streams_request_body_chunks() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let chunks = stream::iter(vec![
        Ok::<Bytes, HttpClientError>(Bytes::from_static(b"hello ")),
        Ok(Bytes::from_static(b"stream")),
    ]);
    let response = client
        .execute(OutboundRequest::post("/echo").body_stream(chunks))
        .await
        .expect("request succeeds");
    let body = response.body.collect(4096).await.expect("body collects");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("echo is JSON");
    assert_eq!(value["body"], "hello stream");
}

#[tokio::test]
async fn injects_traceparent_from_the_request_context() {
    use std::sync::Arc;

    let base = spawn_server().await;
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let body = webx_core::request_context::RequestContext::run_with_trace(
        None,
        Some(Arc::from(trace_id)),
        async move {
            let client = HttpClient::new("test", options(base)).expect("client builds");
            let response = client
                .execute(OutboundRequest::get("/echo"))
                .await
                .expect("request succeeds");
            response.body.collect(4096).await.expect("body collects")
        },
    )
    .await;
    let value: serde_json::Value = serde_json::from_slice(&body).expect("echo is JSON");
    let traceparent = value["traceparent"].as_str().unwrap_or_default();
    assert!(
        traceparent.starts_with(&format!("00-{trace_id}-")),
        "{traceparent}"
    );
    assert!(traceparent.ends_with("-01"), "{traceparent}");
}

#[tokio::test]
async fn decodes_upstream_sse_events() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let response = client
        .execute(OutboundRequest::get("/sse"))
        .await
        .expect("request succeeds");
    let events: Vec<_> = response
        .body
        .sse()
        .map(|event| event.expect("event decodes"))
        .collect()
        .await;

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data, Bytes::from_static(b"one"));
    assert_eq!(events[1].data, Bytes::from_static(b"two"));
}

#[tokio::test]
async fn request_timeout_fires_before_headers() {
    let base = spawn_server().await;
    let client = HttpClient::new(
        "test",
        HttpClientOptions {
            base_url: base,
            request_timeout: Some(Duration::from_millis(50)),
            ..HttpClientOptions::default()
        },
    )
    .expect("client builds");

    let error = client
        .execute(OutboundRequest::get("/slow"))
        .await
        .expect_err("slow response must time out");
    assert!(matches!(error, HttpClientError::Timeout(_)));
}

#[tokio::test]
async fn read_timeout_fires_between_stream_frames() {
    let base = spawn_server().await;
    let client = HttpClient::new(
        "test",
        HttpClientOptions {
            base_url: base,
            read_timeout: Some(Duration::from_millis(50)),
            ..HttpClientOptions::default()
        },
    )
    .expect("client builds");

    let response = client
        .execute(OutboundRequest::get("/slow-body"))
        .await
        .expect("headers arrive");
    let mut events = response.body.sse();

    let first = events.next().await.expect("first event").expect("decodes");
    assert_eq!(first.data, Bytes::from_static(b"one"));

    let error = events
        .next()
        .await
        .expect("second poll")
        .expect_err("times out");
    assert!(matches!(error, HttpClientError::Timeout(_)));
}

#[tokio::test]
async fn collects_only_up_to_the_limit() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let response = client
        .execute(OutboundRequest::get("/big"))
        .await
        .expect("request succeeds");
    let error = response
        .body
        .collect(16)
        .await
        .expect_err("body exceeds the limit");
    assert!(matches!(error, HttpClientError::BodyTooLarge { limit: 16 }));
}

#[tokio::test]
async fn non_2xx_is_a_response_not_an_error() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let response = client
        .execute(OutboundRequest::get("/status500"))
        .await
        .expect("request succeeds");
    assert_eq!(response.status, 500);
    assert!(!response.is_success());
    assert_eq!(
        &response.body.collect(64).await.expect("body collects")[..],
        b"boom"
    );
}

#[tokio::test]
async fn registry_resolves_names() {
    let base = spawn_server().await;
    let client: Arc<dyn IHttpClient> =
        Arc::new(HttpClient::new("test", options(base)).expect("client builds"));

    let registry = HttpClientRegistry::new().with("test", client);
    let factory: &dyn IHttpClientFactory = &registry;

    assert!(factory.create("test").is_ok());
    assert!(matches!(
        factory.create("missing"),
        Err(HttpClientError::UnknownClient(_))
    ));
}

#[tokio::test]
async fn hop_by_hop_headers_are_not_forwarded() {
    let base = spawn_server().await;
    let client = HttpClient::new("test", options(base)).expect("client builds");

    let response = client
        .execute(
            OutboundRequest::get("/echo")
                .header("connection", "close, x-drop")
                .header("keep-alive", "timeout=5")
                .header("transfer-encoding", "chunked")
                .header("proxy-authorization", "Basic abc")
                .header("x-keep", "yes")
                .header("x-drop", "secret"),
        )
        .await
        .expect("request succeeds");
    let body = response.body.collect(4096).await.expect("body collects");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("echo is JSON");

    for name in [
        "connection",
        "keep_alive",
        "transfer_encoding",
        "proxy_authorization",
        "x_drop",
    ] {
        assert_eq!(
            value[name], "",
            "{name} is hop-by-hop and must not reach the upstream"
        );
    }
    assert_eq!(value["x_keep"], "yes", "end-to-end headers still pass");
}

#[tokio::test]
async fn proxy_tunnels_requests_with_connect() {
    let target = spawn_server().await;
    let proxy = spawn_connect_proxy().await;
    let client = HttpClient::new(
        "test",
        HttpClientOptions {
            base_url: target,
            proxy: Some(proxy),
            ..HttpClientOptions::default()
        },
    )
    .expect("client builds with a proxy");

    let response = client
        .execute(OutboundRequest::get("/echo").header("x-request-id", "via-proxy"))
        .await
        .expect("request succeeds through the tunnel");
    let body = response.body.collect(4096).await.expect("body collects");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("echo is JSON");
    assert_eq!(value["request_id"], "via-proxy");
    assert_eq!(value["path"], "/echo");
}

#[tokio::test]
async fn custom_root_certificates_are_loaded_and_bad_files_fail_fast() {
    let dir = tempfile::tempdir().expect("temp dir");
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("generate");
    let path = dir.path().join("ca.pem");
    std::fs::write(&path, cert.cert.pem()).expect("write certificate");

    HttpClient::new(
        "tls",
        HttpClientOptions {
            base_url: "https://localhost".into(),
            root_certificates: Some(path),
            ..HttpClientOptions::default()
        },
    )
    .expect("a generated PEM bundle is accepted");

    let missing = dir.path().join("missing.pem");
    let error = HttpClient::new(
        "tls",
        HttpClientOptions {
            base_url: "https://localhost".into(),
            root_certificates: Some(missing),
            ..HttpClientOptions::default()
        },
    )
    .err()
    .expect("a missing certificate file must fail construction");
    assert!(matches!(error, HttpClientError::InvalidRequest(_)));

    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, b"not a certificate").expect("write empty");
    let error = HttpClient::new(
        "tls",
        HttpClientOptions {
            base_url: "https://localhost".into(),
            root_certificates: Some(empty),
            ..HttpClientOptions::default()
        },
    )
    .err()
    .expect("a PEM file without certificates must fail construction");
    assert!(matches!(error, HttpClientError::InvalidRequest(_)));
}
