//! Presence transitions: the registry reports first/last connection atomically.
//!
//! Business code observes `IPresenceObserver` instead of counting connections
//! across `register` + `user_connection_count`, which is racy: two concurrent
//! connections could each see "not the first".

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jsonwebtoken::{EncodingKey, Header};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use webx::*;

const JWT_SECRET: &str = "presence-observer-test-secret-at-least-32-bytes";

struct QuietHandler;

#[async_trait]
impl IWebSocketHandler for QuietHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        while let Some(message) = connection.recv().await {
            if matches!(message?, RealtimeMessage::Close { .. }) {
                break;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Recorder {
    transitions: Mutex<Vec<PresenceTransition>>,
}

impl Recorder {
    fn snapshot(&self) -> Vec<PresenceTransition> {
        self.transitions
            .lock()
            .expect("recorder not poisoned")
            .clone()
    }
}

#[async_trait]
impl IPresenceObserver for Recorder {
    async fn on_presence_transition(&self, transition: PresenceTransition) {
        self.transitions
            .lock()
            .expect("recorder not poisoned")
            .push(transition);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn alice_token() -> String {
    #[derive(serde::Serialize)]
    struct Claims {
        sub: String,
        tenant_id: String,
        exp: usize,
    }
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as usize
        + 3600;
    jsonwebtoken::encode(
        &Header::default(),
        &Claims {
            sub: "alice".into(),
            tenant_id: "acme".into(),
            exp,
        },
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("token encodes")
}

async fn spawn_host(port: u16, recorder: Arc<Recorder>) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_authentication()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = JWT_SECRET.to_string();
            });
        })
        .use_websocket("/ws", Arc::new(QuietHandler))
        .use_presence_observer(recorder)
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return handle;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(port: u16, token: &str) -> Client {
    let mut request = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("bearer header"),
    );
    let (client, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("handshake");
    assert_eq!(response.status(), 101);
    client
}

async fn wait_for(recorder: &Recorder, expected: usize) -> Vec<PresenceTransition> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let transitions = recorder.snapshot();
        if transitions.len() >= expected {
            return transitions;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected} transitions, got {}: {transitions:?}",
            transitions.len()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_transitions_report_first_and_last_connection() {
    let port = free_port();
    let recorder = Arc::new(Recorder::default());
    let handle = spawn_host(port, Arc::clone(&recorder)).await;
    let token = alice_token();

    let mut first = connect(port, &token).await;
    let transitions = wait_for(&recorder, 1).await;
    assert_eq!(transitions[0].user_id, "alice");
    assert_eq!(transitions[0].tenant_id.as_deref(), Some("acme"));
    assert_eq!(transitions[0].direction, PresenceDirection::Connected);
    assert!(transitions[0].is_first());

    let mut second = connect(port, &token).await;
    let transitions = wait_for(&recorder, 2).await;
    assert_eq!(transitions[1].connections, 2);
    assert!(!transitions[1].is_first());

    first.close(None).await.expect("close first");
    let transitions = wait_for(&recorder, 3).await;
    assert_eq!(transitions[2].direction, PresenceDirection::Disconnected);
    assert_eq!(transitions[2].connections, 1);
    assert!(!transitions[2].is_last());

    second.close(None).await.expect("close second");
    let transitions = wait_for(&recorder, 4).await;
    assert_eq!(transitions[3].direction, PresenceDirection::Disconnected);
    assert_eq!(transitions[3].connections, 0);
    assert!(transitions[3].is_last());

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_connections_do_not_emit_presence() {
    let port = free_port();
    let recorder = Arc::new(Recorder::default());
    let handle = spawn_host(port, Arc::clone(&recorder)).await;

    let (mut client, response) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("handshake");
    assert_eq!(response.status(), 101);
    // The session is up, but an anonymous connection has no identity to report.
    client.close(None).await.expect("close");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(recorder.snapshot().is_empty());

    handle.shutdown();
}
