//! In-process wire harness: protocol bytes through the real engine, no port.
//!
//! `TestHost::dispatch` covers handler/DI/mediator behaviour. This suite covers
//! the byte-level path instead: HTTP/1.1 keep-alive, h2c prior knowledge,
//! malformed input recovery and SSE framing — all over a tokio duplex pipe that
//! drives the real engine connection loop (hyper auto builder, router,
//! middleware, error writers, drain handling). No TCP listener is bound, so the
//! tests are deterministic and fast; the design mirrors quiche's in-memory
//! `Pipe` client/server test pair.
//!
//! The harness is the second half of `docs/design/test-host.md`: `dispatch`
//! tests handlers, `connect_wire` tests protocol bytes.

#![allow(non_snake_case)]

use std::time::Duration;

use serde::{Deserialize, Serialize};
use webx::*;

const WIRE_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Routes served over the in-process wire
// ---------------------------------------------------------------------------

#[derive(Debug, Default, WebxRequestMeta)]
struct WireHelloRequest;

#[get("/wire/hello")]
impl IRequest<WireHelloReply> for WireHelloRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct WireHelloReply {
    message: &'static str,
}

#[derive(Default)]
struct WireHelloHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<WireHelloRequest, WireHelloReply> for WireHelloHandler {
    async fn handle(&mut self, _req: WireHelloRequest) -> ActionResult<WireHelloReply> {
        Ok(WireHelloReply {
            message: "hello wire",
        })
    }
}

#[derive(Debug, Default, WebxRequestMeta)]
struct WireSseRequest;

#[get("/wire/sse")]
impl IRequest<Response> for WireSseRequest {}

#[derive(Default)]
struct WireSseHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<WireSseRequest, Response> for WireSseHandler {
    async fn handle(&mut self, _req: WireSseRequest) -> ActionResult<Response> {
        let events =
            futures_util::stream::iter([Ok(SseEvent::data("hello")), Ok(SseEvent::data("done"))]);
        Ok(Box::new(SseResult::new(events)))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn wire_host() -> TestHost {
    TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa())
}

fn status_of(response: &[u8]) -> u16 {
    let text = String::from_utf8_lossy(response);
    text.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("response starts with an HTTP status line")
}

/// Read exactly one HTTP/1.1 response: headers, then `Content-Length` bytes.
async fn read_one_http11_response(conn: &mut WireConnection) -> Vec<u8> {
    use tokio::io::AsyncReadExt;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let read = tokio::time::timeout(WIRE_TIMEOUT, conn.stream.read(&mut chunk))
            .await
            .expect("response header read timed out")
            .expect("read response header");
        assert!(read > 0, "connection closed before response headers");
        buf.extend_from_slice(&chunk[..read]);
        if let Some(position) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };

    let content_length = std::str::from_utf8(&buf[..header_end])
        .ok()
        .and_then(|head| {
            head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .unwrap_or(0);

    while buf.len() < header_end + content_length {
        let read = tokio::time::timeout(WIRE_TIMEOUT, conn.stream.read(&mut chunk))
            .await
            .expect("response body read timed out")
            .expect("read response body");
        assert!(read > 0, "connection closed before response body");
        buf.extend_from_slice(&chunk[..read]);
    }
    buf.truncate(header_end + content_length);
    buf
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http11_keep_alive_reuses_one_pipe() {
    let host = wire_host();
    let mut conn = host.connect_wire().await;

    conn.write(b"GET /wire/hello HTTP/1.1\r\nhost: wire\r\n\r\n")
        .await
        .expect("write first request");
    let first = read_one_http11_response(&mut conn).await;
    assert_eq!(status_of(&first), 200);
    assert!(
        String::from_utf8_lossy(&first).contains("hello wire"),
        "unexpected first response: {:?}",
        String::from_utf8_lossy(&first)
    );

    // The same pipe carries the second request without a new connection: this
    // is exactly the keep-alive path a TCP listener would serve.
    conn.write(b"GET /wire/hello HTTP/1.1\r\nhost: wire\r\nconnection: close\r\n\r\n")
        .await
        .expect("write second request");
    let second = tokio::time::timeout(WIRE_TIMEOUT, conn.read_at_most(64 * 1024))
        .await
        .expect("second response timed out")
        .expect("read second response");
    assert_eq!(status_of(&second), 200);
    assert!(String::from_utf8_lossy(&second).contains("hello wire"));

    conn.close().await;
}

#[tokio::test]
async fn h2c_prior_knowledge_works_over_the_pipe() {
    use http_body_util::BodyExt;
    use hyper_util::rt::{TokioExecutor, TokioIo};

    let host = wire_host();
    let WireConnection { stream, task } = host.connect_wire().await;

    // hyper's auto builder detects the HTTP/2 preface when no ALPN negotiated,
    // so the same in-process pipe can exercise the h2c path.
    let (mut sender, connection) = hyper::client::conn::http2::handshake::<
        _,
        _,
        http_body_util::Empty<bytes::Bytes>,
    >(TokioExecutor::new(), TokioIo::new(stream))
    .await
    .expect("HTTP/2 handshake over the pipe");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = hyper::Request::builder()
        .method("GET")
        .uri("/wire/hello")
        .header("host", "wire")
        .body(http_body_util::Empty::new())
        .expect("build h2 request");
    let response = tokio::time::timeout(WIRE_TIMEOUT, sender.send_request(request))
        .await
        .expect("h2 request timed out")
        .expect("h2 request");
    assert_eq!(response.status(), 200);
    assert_eq!(response.version(), hyper::Version::HTTP_2);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect h2 body")
        .to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("hello wire"));

    drop(sender);
    let _ = tokio::time::timeout(WIRE_TIMEOUT, task).await;
}

#[tokio::test]
async fn malformed_request_is_rejected_and_host_recovers() {
    use tokio::io::AsyncReadExt;

    let host = wire_host();
    let mut conn = host.connect_wire().await;
    conn.write(b"THIS IS NOT HTTP\r\n\r\n")
        .await
        .expect("write malformed request");

    let mut chunk = [0u8; 1024];
    let outcome =
        tokio::time::timeout(Duration::from_millis(500), conn.stream.read(&mut chunk)).await;
    match outcome {
        // Closing without a response is an acceptable rejection.
        Ok(Ok(0)) => {}
        Ok(Ok(read)) => {
            let text = String::from_utf8_lossy(&chunk[..read]);
            assert!(
                text.starts_with("HTTP/1.1 400") || text.starts_with("HTTP/1.0 400"),
                "malformed request must be rejected with 400, got {text:?}"
            );
        }
        Ok(Err(error)) => panic!("reading the rejection failed: {error}"),
        Err(_) => panic!("server neither rejected nor closed the malformed request"),
    }
    conn.close().await;

    // The malformed input only affected its own connection: a fresh pipe still
    // serves routed requests.
    let mut fresh = host.connect_wire().await;
    fresh
        .write(b"GET /wire/hello HTTP/1.1\r\nhost: wire\r\nconnection: close\r\n\r\n")
        .await
        .expect("write recovery request");
    let response = tokio::time::timeout(WIRE_TIMEOUT, fresh.read_at_most(64 * 1024))
        .await
        .expect("recovery response timed out")
        .expect("read recovery response");
    assert_eq!(status_of(&response), 200);
    fresh.close().await;
}

#[tokio::test]
async fn sse_frames_stream_over_the_pipe() {
    let host = wire_host();
    let mut conn = host.connect_wire().await;
    conn.write(
        b"GET /wire/sse HTTP/1.1\r\nhost: wire\r\naccept: text/event-stream\r\nconnection: close\r\n\r\n",
    )
    .await
    .expect("write SSE request");

    let response = tokio::time::timeout(WIRE_TIMEOUT, conn.read_at_most(64 * 1024))
        .await
        .expect("SSE response timed out")
        .expect("read SSE response");
    let text = String::from_utf8_lossy(&response);
    assert_eq!(status_of(&response), 200, "unexpected SSE status: {text:?}");
    assert!(
        text.contains("data:hello"),
        "missing first SSE frame: {text:?}"
    );
    assert!(
        text.contains("data:done"),
        "missing terminal SSE frame: {text:?}"
    );
    conn.close().await;
}
