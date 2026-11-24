//! WebSocket close semantics: every framework-initiated end records a
//! [`CloseReason`] on the connection and sends the matching RFC 6455 close code.
//!
//! These are host-level tests: `HostBuilder::use_websocket` takes the handler
//! directly, so no inventory routes (and no umbrella-crate dependency) are
//! needed to exercise the real session lifecycle.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use webx_core::error::Result;
use webx_core::mode::AppMode;
use webx_core::realtime::{
    CloseReason, IConnection, IWebSocketHandler, RealtimeMessage, WebSocketLimits,
};
use webx_host::server::{Host, ServerHandle};

const TEST_SECRET: &str = "close-semantics-test-secret-key-0123456789";

/// Serializes the four session tests in this binary.
///
/// Each test binds an ephemeral port, starts its own host and connects a WS
/// client to the **same** `/ws` path. Running them concurrently lets the
/// port-probe → `run_at` window hand a client to another test's host, which
/// then keeps the connection open after the "wrong" host is shut down. The
/// tests exercise process-local session semantics, so serializing them costs a
/// few seconds and removes the cross-wiring entirely.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

async fn spawn_host(host: Host, port: u16) -> (ServerHandle, tokio::task::JoinHandle<()>) {
    let handle = host.server_handle();
    let address = format!("127.0.0.1:{port}");
    let server = tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    (handle, server)
}

async fn shutdown_and_join(handle: ServerHandle, server: tokio::task::JoinHandle<()>) {
    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("host must stop within the test timeout")
        .expect("host task must not panic");
}

/// Connection slot the handler publishes so the test can inspect
/// [`IConnection::close_reason`] after the session ended.
type ConnectionSlot = Arc<Mutex<Option<Arc<dyn IConnection>>>>;

/// Echo handler that records its connection handle, then serves the session
/// until the framework or the peer ends it.
struct Recorder {
    slot: ConnectionSlot,
}

impl Recorder {
    fn new() -> (Arc<Self>, ConnectionSlot) {
        let slot: ConnectionSlot = Arc::new(Mutex::new(None));
        (
            Arc::new(Self {
                slot: Arc::clone(&slot),
            }),
            slot,
        )
    }
}

#[async_trait::async_trait]
impl IWebSocketHandler for Recorder {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        *self
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&connection));
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

fn websocket_host(handler: Arc<Recorder>, limits: WebSocketLimits) -> Host {
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/ws", handler)
        .use_websocket_limits(limits)
        .build()
}

type Client = tokio_tungstenite::WebSocketStream<TcpStream>;

async fn connect(port: u16) -> Client {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("client connect");
    let request = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .expect("upgrade request");
    let (client, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("websocket handshake");
    assert_eq!(response.status(), 101);
    client
}

async fn connect_with_bearer(port: u16, token: &str) -> Client {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("client connect");
    let mut request = format!("ws://127.0.0.1:{port}/ws")
        .into_client_request()
        .expect("upgrade request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid authorization header"),
    );
    let (client, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("websocket handshake");
    assert_eq!(response.status(), 101);
    client
}

/// Wait until the handler published its connection (the session is live).
async fn wait_for_connection(slot: &ConnectionSlot) -> Arc<dyn IConnection> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(connection) = slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return connection;
        }
        assert!(
            Instant::now() < deadline,
            "the handler never received a connection"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait until the framework recorded `expected` as the close reason.
async fn wait_for_reason(slot: &ConnectionSlot, expected: CloseReason) -> Arc<dyn IConnection> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(connection) = slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            if connection.close_reason() == Some(expected) {
                return connection;
            }
            assert!(
                Instant::now() < deadline,
                "expected close reason {expected:?}, got {:?}",
                connection.close_reason()
            );
        } else {
            assert!(
                Instant::now() < deadline,
                "the handler never received a connection"
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Assert the client observes the server's close handshake: a close frame or
/// end-of-stream. Used when the peer's own close code is the interesting part
/// (the server echoes it instead of choosing one).
async fn expect_server_close(client: &mut Client) {
    let next = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("close handshake deadline");
    match next {
        None | Some(Ok(Message::Close(_))) | Some(Err(_)) => {}
        Some(Ok(other)) => panic!("expected the server to close, got {other:?}"),
    }
}

/// Assert the client observes a close frame with `expected_code`.
async fn expect_close_frame(client: &mut Client, expected_code: u16) {
    let next = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("close frame deadline");
    match next {
        Some(Ok(Message::Close(Some(frame)))) => assert_eq!(
            u16::from(frame.code),
            expected_code,
            "unexpected close code (server reason: {:?})",
            frame.reason
        ),
        Some(Ok(Message::Close(None))) => panic!("the close frame carried no code"),
        Some(Ok(other)) => panic!("expected a close frame, got {other:?}"),
        Some(Err(error)) => panic!("transport error instead of a close frame: {error}"),
        None => panic!("the stream ended without a close frame"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_close_is_recorded_as_client_closed() {
    let _serial = SERIAL.lock().await;
    let port = free_port();
    let (handler, slot) = Recorder::new();
    let (shutdown, server) =
        spawn_host(websocket_host(handler, WebSocketLimits::default()), port).await;
    let mut client = connect(port).await;
    wait_for_connection(&slot).await;

    client.close(None).await.expect("client close");

    wait_for_reason(&slot, CloseReason::ClientClosed).await;
    // The server answers the peer's close handshake (the peer sent no code, so
    // tungstenite echoes an empty close frame rather than a server-chosen code).
    expect_server_close(&mut client).await;

    shutdown_and_join(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifetime_deadline_is_recorded_as_lifetime_exceeded() {
    let _serial = SERIAL.lock().await;
    let port = free_port();
    let (handler, slot) = Recorder::new();
    let limits =
        WebSocketLimits::default().with_max_connection_lifetime(Duration::from_millis(200));
    let (shutdown, server) = spawn_host(websocket_host(handler, limits), port).await;
    let mut client = connect(port).await;
    wait_for_connection(&slot).await;

    // Traffic does not extend the absolute deadline; the client sees 1001.
    expect_close_frame(&mut client, 1001).await;
    wait_for_reason(&slot, CloseReason::LifetimeExceeded).await;

    shutdown_and_join(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_expiry_is_recorded_as_token_expired() {
    use serde::Serialize;

    #[derive(Serialize)]
    struct Claims {
        sub: String,
        exp: usize,
    }

    fn token_expiring_in(seconds: u64) -> String {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs()
            + seconds;
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &Claims {
                sub: "ws-user".to_string(),
                exp: exp as usize,
            },
            &jsonwebtoken::EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .expect("token")
    }

    let _serial = SERIAL.lock().await;
    let port = free_port();
    let (handler, slot) = Recorder::new();
    let limits = WebSocketLimits::default().close_at_token_expiry();
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_authentication()
        .configure(|builder| {
            builder.use_options(|options| {
                options.jwt.secret = TEST_SECRET.to_string();
            });
        })
        .use_websocket("/ws", handler)
        .use_websocket_limits(limits)
        .build();
    let (shutdown, server) = spawn_host(host, port).await;

    let mut client = connect_with_bearer(port, &token_expiring_in(2)).await;
    let connection = wait_for_connection(&slot).await;
    assert_eq!(
        connection.context().identity().map(|user| user.id.as_str()),
        Some("ws-user"),
        "the upgrade must be authenticated for the token expiry to be tracked"
    );

    // The session outlives the access token: the client sees 1008.
    expect_close_frame(&mut client, 1008).await;
    wait_for_reason(&slot, CloseReason::TokenExpired).await;

    shutdown_and_join(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_shutdown_is_recorded_as_server_shutdown() {
    let _serial = SERIAL.lock().await;
    let port = free_port();
    let (handler, slot) = Recorder::new();
    let (shutdown, server) =
        spawn_host(websocket_host(handler, WebSocketLimits::default()), port).await;
    let mut client = connect(port).await;
    wait_for_connection(&slot).await;

    shutdown.shutdown();

    // The session ends proactively with 1001 ("going away") before the engine
    // drain deadline, instead of being cut off when the process exits.
    expect_close_frame(&mut client, 1001).await;
    wait_for_reason(&slot, CloseReason::ServerShutdown).await;

    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("host must stop within the test timeout")
        .expect("host task must not panic");
}
