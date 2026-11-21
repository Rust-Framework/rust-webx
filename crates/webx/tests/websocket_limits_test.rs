//! 2.3-5 connection-level limits: frame size, message rate, idle timeout.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
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

#[async_trait]
impl IWebSocketHandler for EchoHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
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

async fn spawn_host(port: u16, limits: WebSocketLimits) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/echo", Arc::new(EchoHandler))
        .use_websocket_limits(limits)
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    handle
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect(port: u16) -> Client {
    let (client, response) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/echo"))
            .await
            .expect("handshake");
    assert_eq!(response.status(), 101);
    client
}

async fn recv_json(client: &mut Client) -> serde_json::Value {
    let message = client.next().await.expect("frame").expect("frame ok");
    match message {
        Message::Text(text) => serde_json::from_str(&text).expect("json"),
        other => panic!("expected text frame, got {other:?}"),
    }
}

/// Accept the normal ways a server-initiated close reaches the client: a close
/// frame, end-of-stream, or a transport error.
fn assert_server_closed(
    next: Option<std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>,
) {
    match next {
        None => {}
        Some(Err(_)) => {}
        Some(Ok(Message::Close(_))) => {}
        Some(Ok(other)) => panic!("expected the server to close, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_frame_is_rejected_but_the_session_survives() {
    let port = free_port();
    let handle = spawn_host(port, WebSocketLimits::default().with_max_frame_bytes(8)).await;
    let mut client = connect(port).await;

    client
        .send(Message::Text("123456789012".into()))
        .await
        .expect("send oversized");
    let failure = recv_json(&mut client).await;
    assert_eq!(failure["error"]["code"], "frame_too_large");

    client
        .send(Message::Text("ok".into()))
        .await
        .expect("send small");
    let reply = client.next().await.expect("reply").expect("frame");
    assert_eq!(reply, Message::Text("ok".into()));

    client.close(None).await.expect("close");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn message_rate_limit_closes_the_connection_after_an_error_frame() {
    let port = free_port();
    let handle = spawn_host(
        port,
        WebSocketLimits::default().with_max_messages_per_second(2),
    )
    .await;
    let mut client = connect(port).await;

    for text in ["one", "two"] {
        client
            .send(Message::Text(text.into()))
            .await
            .expect("send within limit");
        assert_eq!(
            client.next().await.expect("echo").expect("frame"),
            Message::Text(text.into())
        );
    }

    // Third frame in the same one-second window trips the limit.
    client
        .send(Message::Text("three".into()))
        .await
        .expect("send over limit");
    let failure = recv_json(&mut client).await;
    assert_eq!(failure["error"]["code"], "rate_limited");

    // The server closed the connection; the client observes a close.
    let next = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("close deadline");
    assert_server_closed(next);

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_connection_is_closed_by_the_timeout() {
    let port = free_port();
    let handle = spawn_host(
        port,
        WebSocketLimits::default().with_idle_timeout(Duration::from_millis(200)),
    )
    .await;
    let mut client = connect(port).await;
    // Send nothing: the server must close the idle session.
    let next = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("idle close deadline");
    assert_server_closed(next);
    handle.shutdown();
}
