//! 2.3-4a end-to-end: connection registry fan-out + presence.
//!
//! The host registers an in-memory registry for WebSocket routes; sessions
//! register themselves on connect and unregister on teardown, so a handler can
//! fan out to a room or query presence without owning other sockets.

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

/// Mini chat/room protocol over the connection:
/// `join:<room>`, `say:<room>`, `count`, `presence:<user>`, `away`.
struct RoomHandler;

#[async_trait]
impl IWebSocketHandler for RoomHandler {
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
            } else if let Some(room) = text.strip_prefix("say:") {
                let peers = registry.room_connections(room);
                for peer in &peers {
                    peer.send_text(format!("room:{room}")).await?;
                }
                connection
                    .send_text(format!("sent:{}", peers.len()))
                    .await?;
            } else if text == "count" {
                connection.send_text(registry.len().to_string()).await?;
            } else if let Some(user) = text.strip_prefix("presence:") {
                let presence = registry.presence(user);
                connection
                    .send_text(format!(
                        "{}:{}",
                        presence.status.as_str(),
                        presence.connections
                    ))
                    .await?;
            } else if text == "away" {
                if let Some(identity) = connection.context().identity() {
                    registry.set_presence(
                        &identity.id,
                        PresenceStatus::Away,
                        Duration::from_secs(30),
                    );
                    connection.send_text("away".to_string()).await?;
                } else {
                    connection.send_text("anonymous".to_string()).await?;
                }
            }
        }
        Ok(())
    }
}

struct TestUserScheme;

#[async_trait]
impl IAuthenticationScheme for TestUserScheme {
    fn name(&self) -> &'static str {
        "test-user"
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        ctx.request().header("x-test-user").is_some()
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        let Some(user) = ctx.request().header("x-test-user").map(str::to_string) else {
            return Ok(None);
        };
        Ok(Some(Box::new(ApiKeyClaims::new(user))))
    }
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect(port: u16, user: Option<&'static str>) -> Client {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    let mut request = format!("ws://127.0.0.1:{port}/room")
        .into_client_request()
        .expect("request");
    if let Some(user) = user {
        request
            .headers_mut()
            .insert("x-test-user", HeaderValue::from_static(user));
    }
    let (client, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("handshake");
    client
}

async fn send(client: &mut Client, text: &str) {
    client
        .send(Message::Text(text.into()))
        .await
        .expect("send frame");
}

async fn recv_text(client: &mut Client) -> String {
    match client.next().await.expect("frame").expect("frame ok") {
        Message::Text(text) => text.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_fans_out_to_room_members_and_tracks_presence() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/room", Arc::new(RoomHandler))
        .add_authentication_scheme(Arc::new(TestUserScheme))
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let mut alice_a = connect(port, Some("alice")).await;
    let mut alice_b = connect(port, Some("alice")).await;
    let mut bob = connect(port, Some("bob")).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    send(&mut bob, "count").await;
    assert_eq!(recv_text(&mut bob).await, "3");

    // Presence: two connections for alice, both online.
    send(&mut bob, "presence:alice").await;
    assert_eq!(recv_text(&mut bob).await, "online:2");

    send(&mut alice_a, "join:r1").await;
    assert_eq!(recv_text(&mut alice_a).await, "joined");
    send(&mut alice_b, "join:r1").await;
    assert_eq!(recv_text(&mut alice_b).await, "joined");

    // Fan-out: both room members receive the broadcast.
    send(&mut alice_a, "say:r1").await;
    assert_eq!(recv_text(&mut alice_a).await, "room:r1");
    assert_eq!(recv_text(&mut alice_a).await, "sent:2");
    assert_eq!(recv_text(&mut alice_b).await, "room:r1");

    // Explicit away marker while still connected.
    send(&mut alice_a, "away").await;
    assert_eq!(recv_text(&mut alice_a).await, "away");
    send(&mut bob, "presence:alice").await;
    assert_eq!(recv_text(&mut bob).await, "away:2");

    // Teardown unregisters sessions.
    alice_a.close(None).await.expect("close a");
    alice_b.close(None).await.expect("close b");
    for _ in 0..100 {
        send(&mut bob, "count").await;
        if recv_text(&mut bob).await == "1" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    send(&mut bob, "count").await;
    assert_eq!(recv_text(&mut bob).await, "1");

    send(&mut bob, "presence:alice").await;
    assert_eq!(recv_text(&mut bob).await, "offline:0");

    bob.close(None).await.expect("close bob");
    handle.shutdown();
}
