//! 2.3-4b end-to-end: cross-node real-time fan-out rides the event transport.
//!
//! The framework no longer ships a second pub/sub port. Publishing
//! `RealtimeFanoutEvent` hands the message to the configured event transport
//! (in-memory queue here; Redis/NATS in a deployment), and every node's event
//! consumer routes it through `EventDispatcher` to the framework fan-out
//! handler, which writes it to that node's room connections.
//!
//! This test exercises the same path a remote node would take: the handler is
//! resolved from DI and the payload arrives as encoded event JSON.

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

/// Minimal room protocol: `join:<room>` registers the connection in the
/// registry room; `ready` acknowledges readiness.
struct JoinRoomHandler;

#[async_trait]
impl IWebSocketHandler for JoinRoomHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let registry = connection
            .context()
            .resolve::<dyn IConnectionRegistry>()
            .expect("the host registers an IConnectionRegistry for WebSocket routes");
        while let Some(message) = connection.recv().await {
            let text = match message? {
                RealtimeMessage::Text(text) => text,
                RealtimeMessage::Close { .. } => break,
                RealtimeMessage::Binary(_) => continue,
            };
            if let Some(room) = text.strip_prefix("join:") {
                registry.join(connection.id(), room);
                connection.send_text("joined".to_string()).await?;
            } else if text == "ready" {
                connection.send_text("ready".to_string()).await?;
            }
        }
        Ok(())
    }
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect(port: u16) -> Client {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let request = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .expect("request");
    let (client, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws handshake");
    client
}

async fn send(client: &mut Client, text: &str) {
    client
        .send(Message::Text(text.to_string().into()))
        .await
        .expect("ws send");
}

async fn expect_text(client: &mut Client, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let message = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("frame deadline")
            .expect("frame")
            .expect("frame ok");
        if let Message::Text(text) = message {
            if text == expected {
                return;
            }
        }
    }
    panic!("did not receive {expected:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn realtime_fanout_event_reaches_room_connections_via_the_event_bus() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/ws", Arc::new(JoinRoomHandler))
        .register(|svc| svc.add_mediator())
        .try_build()
        .expect("host must build");

    // Handles must be captured before the host is moved into the server task.
    let mediator = Mediator::new(
        Arc::clone(host.provider()),
        Arc::clone(host.dispatch_runtime().handler_cache()),
    );
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let mut first = connect(port).await;
    let mut second = connect(port).await;
    let mut other_room = connect(port).await;
    send(&mut first, "join:fanout").await;
    expect_text(&mut first, "joined").await;
    send(&mut second, "join:fanout").await;
    expect_text(&mut second, "joined").await;
    send(&mut other_room, "join:other").await;
    expect_text(&mut other_room, "joined").await;

    // Publishing is asynchronous: the event transport accepts it and the
    // worker/fan-out handler delivers to this node's room connections.
    mediator
        .publish(RealtimeFanoutEvent::to_room("fanout", "hello-everyone"))
        .await
        .expect("fan-out event must be accepted by the transport");

    expect_text(&mut first, "hello-everyone").await;
    expect_text(&mut second, "hello-everyone").await;

    // A connection in another room must not receive the fan-out.
    let leaked = tokio::time::timeout(Duration::from_millis(300), other_room.next()).await;
    assert!(
        leaked.is_err(),
        "a connection in another room received the fan-out"
    );

    first.close(None).await.ok();
    second.close(None).await.ok();
    other_room.close(None).await.ok();
    handle.shutdown();
}
