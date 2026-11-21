//! 2.3-1b end-to-end WebSocket session test.
//!
//! Uses a real client (`tokio-tungstenite`) against a real host: upgrade
//! handshake, text/binary echo through the transport-neutral
//! [`webx::IConnection`], `426` for plain HTTP on a WS path, and normal routing
//! (404) when upgrading a path that has no WS route.

use std::sync::atomic::{AtomicU32, Ordering};
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

struct EchoHandler;

#[async_trait::async_trait]
impl IWebSocketHandler for EchoHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        assert_eq!(connection.meta().transport, TransportKind::WebSocket);
        while let Some(message) = connection.recv().await {
            match message? {
                RealtimeMessage::Text(text) => connection.send_text(text).await?,
                RealtimeMessage::Binary(bytes) => connection.send_binary(bytes).await?,
                RealtimeMessage::Close { .. } => break,
            }
        }
        Ok(())
    }
}

async fn spawn_host(port: u16) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/ws", Arc::new(EchoHandler))
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    handle
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_echo_round_trips_text_and_binary() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let (mut client, response) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("websocket handshake");
    assert_eq!(response.status(), 101);

    client
        .send(Message::Text("hello-ws".into()))
        .await
        .expect("send text");
    let echoed = client.next().await.expect("text echo").expect("text frame");
    assert_eq!(echoed, Message::Text("hello-ws".into()));

    client
        .send(Message::Binary(vec![1u8, 2, 3, 4].into()))
        .await
        .expect("send binary");
    let echoed = client
        .next()
        .await
        .expect("binary echo")
        .expect("binary frame");
    assert_eq!(echoed, Message::Binary(vec![1u8, 2, 3, 4].into()));

    client.close(None).await.expect("client close");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_http_on_a_websocket_path_answers_426() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    stream
        .write_all(b"GET /ws HTTP/1.1\r\nHost: test\r\nconnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await.expect("read");
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head);
    assert!(
        head.starts_with("HTTP/1.1 426"),
        "expected 426, got: {head}"
    );
    assert!(
        head.to_ascii_lowercase().contains("upgrade: websocket"),
        "426 must advertise the expected protocol: {head}"
    );

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrading_an_unregistered_path_fails_the_handshake() {
    let port = free_port();
    let handle = spawn_host(port).await;

    let result = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/nope")).await;
    assert!(
        result.is_err(),
        "upgrading a path without a WS route must not complete the handshake"
    );

    handle.shutdown();
}

/// Dropped when a connection scope is disposed.
static SCOPE_DROPS: AtomicU32 = AtomicU32::new(0);

/// Scoped service: one instance per connection, dropped on session teardown.
#[derive(Default)]
struct MessageCounter {
    count: AtomicU32,
}

impl MessageCounter {
    fn increment(&self) -> u32 {
        self.count.fetch_add(1, Ordering::SeqCst) + 1
    }
}

impl Drop for MessageCounter {
    fn drop(&mut self) {
        SCOPE_DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct CounterHandler;

#[async_trait::async_trait]
impl IWebSocketHandler for CounterHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        // The connection scope is created by the host before the handler runs.
        let counter = connection
            .context()
            .resolve::<MessageCounter>()
            .expect("scoped service must resolve from the connection scope");
        let connection_id = connection.id();
        while let Some(message) = connection.recv().await {
            match message? {
                RealtimeMessage::Text(text) if text == "id" => {
                    connection
                        .send_text(format!("{}", connection_id.value()))
                        .await?;
                }
                RealtimeMessage::Text(_) => {
                    let value = counter.increment();
                    connection.send_text(value.to_string()).await?;
                }
                RealtimeMessage::Close { .. } => break,
                RealtimeMessage::Binary(_) => {}
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_scope_is_shared_across_messages_and_disposed() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| svc.scoped(|_| Arc::new(MessageCounter::default())))
        .use_websocket("/counter", Arc::new(CounterHandler))
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let drops_before = SCOPE_DROPS.load(Ordering::SeqCst);
    let (mut client, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/counter"))
            .await
            .expect("counter handshake");

    client
        .send(Message::Text("inc".into()))
        .await
        .expect("send");
    let first = client.next().await.expect("first reply").expect("frame");
    assert_eq!(first, Message::Text("1".into()));

    // The second message must see the same connection-scoped instance.
    client
        .send(Message::Text("inc".into()))
        .await
        .expect("send");
    let second = client.next().await.expect("second reply").expect("frame");
    assert_eq!(second, Message::Text("2".into()));

    // A second connection has its own counter (per-connection isolation).
    let (mut other, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/counter"))
        .await
        .expect("other handshake");
    other.send(Message::Text("inc".into())).await.expect("send");
    let other_first = other.next().await.expect("other reply").expect("frame");
    assert_eq!(other_first, Message::Text("1".into()));

    client.close(None).await.expect("close a");
    other.close(None).await.expect("close b");

    // Session teardown disposes the connection scope and drops its services.
    for _ in 0..100 {
        if SCOPE_DROPS.load(Ordering::SeqCst) > drops_before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        SCOPE_DROPS.load(Ordering::SeqCst) > drops_before,
        "connection scope disposal must drop scoped services"
    );

    handle.shutdown();
}
