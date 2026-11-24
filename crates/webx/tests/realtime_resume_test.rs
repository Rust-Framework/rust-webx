//! Realtime resume: the framework's replay log contract end to end.
//!
//! A handler resolves `IRealtimeEventStore` from the connection scope, feeds the
//! client's `ResumeRequest` to it and sends the `ResumeFrame` back. The host
//! registers the process-local store by default and accepts an override through
//! `HostBuilder::use_realtime_event_store`.

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

/// Replays whatever the client asks for: each frame is a JSON `ResumeRequest`.
struct ResumeHandler;

#[async_trait]
impl IWebSocketHandler for ResumeHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let store = connection
            .context()
            .resolve::<dyn IRealtimeEventStore>()
            .expect("the host registers an IRealtimeEventStore for WebSocket routes");
        while let Some(message) = connection.recv().await {
            let text = match message? {
                RealtimeMessage::Text(text) => text,
                RealtimeMessage::Close { .. } => break,
                RealtimeMessage::Binary(_) => continue,
            };
            let request: ResumeRequest = match serde_json::from_str(&text) {
                Ok(request) => request,
                Err(error) => {
                    connection.send_text(format!("bad_request:{error}")).await?;
                    continue;
                }
            };
            let frame = store.resume(request).await?;
            let body = serde_json::to_string(&frame)
                .map_err(|error| Error::internal(format!("frame encode failed: {error}")))?;
            connection.send_text(body).await?;
        }
        Ok(())
    }
}

async fn spawn_host(port: u16, store: Option<Arc<dyn IRealtimeEventStore>>) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let mut builder = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/ws", Arc::new(ResumeHandler));
    if let Some(store) = store {
        builder = builder.use_realtime_event_store(store);
    }
    let host = builder.build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    handle
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect(port: u16) -> Client {
    let (client, response) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("handshake");
    assert_eq!(response.status(), 101);
    client
}

async fn resume(client: &mut Client, subscription: &str, cursor: Option<u64>) -> ResumeFrame {
    let request = serde_json::json!({ "subscription": subscription, "cursor": cursor });
    client
        .send(Message::Text(request.to_string().into()))
        .await
        .expect("send resume");
    let frame = client.next().await.expect("frame").expect("frame ok");
    match frame {
        Message::Text(text) => serde_json::from_str(&text).expect("resume frame json"),
        other => panic!("expected a resume frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_replays_retained_events_and_reports_the_cursor() {
    let port = free_port();
    let store = Arc::new(InMemoryRealtimeEventStore::default());
    let first = store
        .append("room-1", "first".to_string())
        .await
        .expect("append");
    let second = store
        .append("room-1", "second".to_string())
        .await
        .expect("append");
    let handle = spawn_host(port, Some(store)).await;

    let mut client = connect(port).await;

    let from_scratch = resume(&mut client, "room-1", None).await;
    assert_eq!(
        from_scratch
            .events
            .iter()
            .map(|event| event.payload.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert_eq!(from_scratch.next_cursor, second.event_id);
    assert!(!from_scratch.truncated);

    let after_first = resume(&mut client, "room-1", Some(first.event_id)).await;
    assert_eq!(after_first.events.len(), 1);
    assert_eq!(after_first.events[0].payload, "second");
    assert_eq!(after_first.next_cursor, second.event_id);

    let caught_up = resume(&mut client, "room-1", Some(second.event_id)).await;
    assert!(caught_up.events.is_empty());
    assert_eq!(caught_up.next_cursor, second.event_id);
    assert!(!caught_up.truncated);

    client.close(None).await.expect("close");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unconfigured_host_still_has_the_in_memory_store() {
    let port = free_port();
    let handle = spawn_host(port, None).await;

    let mut client = connect(port).await;
    let frame = resume(&mut client, "fresh-room", None).await;
    assert!(frame.events.is_empty());
    assert_eq!(frame.next_cursor, 0);
    assert!(!frame.truncated);

    client.close(None).await.expect("close");
    handle.shutdown();
}
