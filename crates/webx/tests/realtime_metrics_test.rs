//! 2.3-6: real-time counters on `/metrics`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

#[derive(serde::Deserialize)]
struct EchoRequest {
    text: String,
}

#[derive(serde::Serialize)]
struct EchoReply {
    text: String,
}

impl IRequest<EchoReply> for EchoRequest {}

#[derive(Default)]
struct EchoHandler;

#[handler]
#[async_trait]
impl IRequestHandler<EchoRequest, EchoReply> for EchoHandler {
    async fn handle(&mut self, req: EchoRequest) -> ActionResult<EchoReply> {
        Ok(EchoReply { text: req.text })
    }
}

async fn scrape(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("metrics connection");
    stream
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: metrics\r\nconnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    String::from_utf8_lossy(&response).into_owned()
}

fn metric_value(body: &str, name: &str) -> Option<f64> {
    body.lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .and_then(|value| value.trim().parse().ok())
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect(port: u16) -> Client {
    let (client, response) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc"))
        .await
        .expect("handshake");
    assert_eq!(response.status(), 101);
    client
}

async fn send_json(client: &mut Client, text: &str) {
    client
        .send(Message::Text(format!(r#"{{"text":"{text}"}}"#).into()))
        .await
        .expect("send");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_counters_are_rendered_on_the_metrics_endpoint() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| options.metrics.enabled = true);
        })
        .register(|svc| svc.add_mediator())
        .use_websocket_request::<EchoRequest, EchoReply>("/rpc")
        .use_websocket_limits(WebSocketLimits::default().with_max_messages_per_second(2))
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    // One session processes two messages, then trips the rate limit and closes.
    let mut first = connect(port).await;
    for text in ["a", "b"] {
        send_json(&mut first, text).await;
        let reply = first.next().await.expect("reply").expect("frame");
        assert!(matches!(reply, Message::Text(_)));
    }
    send_json(&mut first, "c").await;
    let failure = first
        .next()
        .await
        .expect("rate limit frame")
        .expect("frame");
    let failure = match failure {
        Message::Text(text) => text.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(failure.contains("rate_limited"), "{failure}");
    drop(first);
    tokio::time::sleep(Duration::from_millis(150)).await;

    // A second live session keeps the registry gauge above zero during scrape.
    let _second = connect(port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let body = scrape(port).await;
    let opened = metric_value(&body, "realtime_connections_opened_total").expect("opened");
    let closed = metric_value(&body, "realtime_connections_closed_total").expect("closed");
    let active = metric_value(&body, "realtime_connections_active").expect("active");
    let messages_in = metric_value(&body, "realtime_messages_in_total").expect("messages_in");
    let messages_out = metric_value(&body, "realtime_messages_out_total").expect("messages_out");
    let rate_limited = metric_value(&body, "realtime_rate_limited_total").expect("rate_limited");

    assert!(opened >= 2.0, "two sessions opened: {body}");
    assert!(closed >= 1.0, "the rate-limited session closed: {body}");
    assert!(active >= 1.0, "second session is live: {body}");
    assert!(messages_in >= 2.0, "two messages were dispatched: {body}");
    assert!(messages_out >= 2.0, "two replies were queued: {body}");
    assert!(rate_limited >= 1.0, "rate limit counted: {body}");
    assert!(
        body.contains("realtime_connection_closes_total{code=\"1008\"}"),
        "rate-limit close code rendered: {body}"
    );
    let dispatch_count =
        metric_value(&body, "realtime_message_dispatch_seconds_count").expect("dispatch count");
    assert!(dispatch_count >= 2.0, "dispatch latency recorded: {body}");
    assert!(
        body.contains("realtime_message_dispatch_seconds_bucket"),
        "dispatch histogram rendered: {body}"
    );

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_snapshot_is_mirrored_into_the_app_meter_on_scrape() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let meter = Arc::new(MemoryMeter::new());
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| options.metrics.enabled = true);
        })
        .use_meter(Arc::clone(&meter) as Arc<dyn IMeter>)
        .register(|svc| svc.add_mediator())
        .use_websocket_request::<EchoRequest, EchoReply>("/rpc")
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let mut client = connect(port).await;
    send_json(&mut client, "hello").await;
    let reply = client.next().await.expect("reply").expect("frame");
    assert!(matches!(reply, Message::Text(_)));

    let body = scrape(port).await;
    assert!(
        body.contains("realtime_messages_in_total"),
        "Prometheus rendering must stay authoritative: {body}"
    );

    // The same snapshot is exported into the registered IMeter on scrape, so an
    // OpenTelemetry/custom exporter sees the real-time counters without a
    // second metrics model.
    let tags: &[(&str, &str)] = &[];
    assert!(
        meter.counter_value("realtime_messages_in_total", tags) >= 1,
        "the app meter must receive the mirrored counters"
    );
    assert!(
        meter.counter_value("realtime_connections_opened_total", tags) >= 1,
        "connection counters must be mirrored too"
    );

    client.close(None).await.ok();
    handle.shutdown();
}
