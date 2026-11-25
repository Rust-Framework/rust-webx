//! 2.9.2 end-to-end: live-connection session revalidation, cookie sessions,
//! shared presence and user-targeted fan-out.
//!
//! Every test runs a real host and a real `tokio-tungstenite` client: the
//! behavior under test (close 1008 after revocation, presence counts, user
//! fan-out) only exists across the HTTP pipeline, the WebSocket session and the
//! event transport, so a unit test could not observe it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use webx::*;

const TEST_SECRET: &str = "test-secret-key-for-live-session-revalidation-0123456789";

fn test_secret() -> &'static str {
    TEST_SECRET
}

fn issuer() -> JwtTokenIssuer {
    JwtTokenIssuer::from_secret(test_secret(), Duration::from_secs(3600))
}

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

async fn spawn_host<F>(port: u16, configure: F) -> ServerHandle
where
    F: FnOnce(HostBuilder) -> HostBuilder,
{
    spawn_host_with(port, Arc::new(EchoHandler), configure).await
}

async fn spawn_host_with<F>(
    port: u16,
    handler: Arc<dyn IWebSocketHandler>,
    configure: F,
) -> ServerHandle
where
    F: FnOnce(HostBuilder) -> HostBuilder,
{
    let address = format!("127.0.0.1:{port}");
    let host = configure(
        Host::builder()
            .mode(AppMode::Development)
            .no_spa()
            .use_websocket("/ws", handler),
    )
    .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    handle
}

type TestClient = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect_with_header(
    port: u16,
    name: &'static str,
    value: String,
) -> std::result::Result<
    (
        TestClient,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    {
        let mut request = format!("ws://127.0.0.1:{port}/ws")
            .into_client_request()
            .expect("ws request");
        request
            .headers_mut()
            .insert(name, HeaderValue::from_str(&value).expect("header value"));
        tokio_tungstenite::connect_async(request).await
    }
}

async fn await_close(client: &mut TestClient) -> (u16, String) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, client.next())
            .await
            .expect("close frame deadline")
            .expect("session ended without a close frame")
            .expect("wire error");
        if let Message::Close(Some(frame)) = frame {
            return (u16::from(frame.code), frame.reason.to_string());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoking_the_jwt_closes_the_live_session_with_1008() {
    let port = free_port();
    let store = Arc::new(InMemorySessionStore::new());
    let store_for_host: Arc<dyn ISessionStore> = store.clone();
    let handle = spawn_host(port, move |builder| {
        builder
            .use_authentication()
            .use_session_store(store_for_host)
            .configure(|app| {
                app.options(|options| options.jwt.secret = test_secret().to_string());
            })
            .use_websocket_limits(
                WebSocketLimits::default().with_session_revalidation(Duration::from_millis(100)),
            )
    })
    .await;

    let issued = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue token");
    let (mut client, response) =
        connect_with_header(port, "authorization", format!("Bearer {}", issued.token))
            .await
            .expect("ws handshake");
    assert_eq!(response.status(), 101);

    store.revoke(&issued.jti).await.expect("revoke");

    let (code, reason) = await_close(&mut client).await;
    assert_eq!(code, 1008, "revocation must use policy-violation close");
    assert_eq!(reason, "session revoked");
    handle.shutdown();
}

/// A store that reports one transient error and then answers "not revoked".
struct FlakySessionStore {
    calls: AtomicU32,
}

#[async_trait::async_trait]
impl ISessionStore for FlakySessionStore {
    async fn revoke(&self, _jti: &str) -> Result<()> {
        Ok(())
    }

    async fn is_revoked(&self, _jti: &str) -> Result<bool> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        // The upgrade itself performs the first check; the second one (the
        // first revalidation tick) is a storage blip, after which the store is
        // healthy again.
        if call == 1 {
            return Err(Error::status(503, "session store blip".to_string()));
        }
        Ok(false)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transient_store_error_does_not_kill_a_live_session() {
    let port = free_port();
    let store = Arc::new(FlakySessionStore {
        calls: AtomicU32::new(0),
    });
    let store_for_host: Arc<dyn ISessionStore> = store.clone();
    let handle = spawn_host(port, move |builder| {
        builder
            .use_authentication()
            .use_session_store(store_for_host)
            .configure(|app| {
                app.options(|options| options.jwt.secret = test_secret().to_string());
            })
            .use_websocket_limits(
                WebSocketLimits::default().with_session_revalidation(Duration::from_millis(80)),
            )
    })
    .await;

    let issued = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue token");
    let (mut client, response) =
        connect_with_header(port, "authorization", format!("Bearer {}", issued.token))
            .await
            .expect("ws handshake");
    assert_eq!(response.status(), 101);

    // Let the first revalidation tick hit the transient error, then prove the
    // session still echoes.
    tokio::time::sleep(Duration::from_millis(350)).await;
    client
        .send(Message::Text("still-here".into()))
        .await
        .expect("send");
    let echoed = tokio::time::timeout(Duration::from_secs(3), client.next())
        .await
        .expect("echo deadline")
        .expect("echo frame")
        .expect("echo payload");
    assert_eq!(echoed, Message::Text("still-here".into()));
    client.close(None).await.expect("close");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_the_cookie_session_closes_the_live_session() {
    let port = free_port();
    let repository = Arc::new(InMemorySessionRepository::new());
    let scheme = Arc::new(
        SessionCookieScheme::new(repository.clone(), CookieOptions::default())
            .expect("cookie scheme"),
    );
    let issued = scheme.issue_for("user-1").await.expect("issue session");
    let cookie_name = scheme.options().name.clone();
    let scheme_for_host: Arc<dyn IAuthenticationScheme> = scheme.clone();
    let handle = spawn_host(port, move |builder| {
        builder
            .add_authentication_scheme(scheme_for_host)
            .use_csrf(CsrfConfig::double_submit())
            .use_websocket_limits(
                WebSocketLimits::default().with_session_revalidation(Duration::from_millis(100)),
            )
    })
    .await;

    let (mut client, response) = connect_with_header(
        port,
        "cookie",
        format!("{cookie_name}={}", issued.session_id),
    )
    .await
    .expect("ws handshake");
    assert_eq!(response.status(), 101);

    scheme
        .revoke_session(&issued.session_id)
        .await
        .expect("delete session");
    let (code, reason) = await_close(&mut client).await;
    assert_eq!(code, 1008);
    assert_eq!(reason, "session revoked");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_presence_counts_one_user_per_node_and_drops_after_disconnect() {
    let port = free_port();
    let presence = Arc::new(InMemoryOnlinePresence::default());
    let presence_for_host = Arc::clone(&presence);
    let handle = spawn_host(port, move |builder| {
        builder
            .use_authentication()
            .configure(|app| {
                app.options(|options| options.jwt.secret = test_secret().to_string());
            })
            .use_online_presence(presence_for_host)
    })
    .await;

    let first = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue first");
    let second = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue second");
    let (first_client, _) =
        connect_with_header(port, "authorization", format!("Bearer {}", first.token))
            .await
            .expect("first handshake");
    let (second_client, _) =
        connect_with_header(port, "authorization", format!("Bearer {}", second.token))
            .await
            .expect("second handshake");

    let scope = PresenceScope::user("user-1");
    let deadline = Instant::now() + Duration::from_secs(5);
    while presence.count(&scope).await.expect("count") != 1 {
        assert!(Instant::now() < deadline, "presence was never published");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    drop(first_client);
    drop(second_client);
    let deadline = Instant::now() + Duration::from_secs(5);
    while presence.count(&scope).await.expect("count") != 0 {
        assert!(Instant::now() < deadline, "presence was never removed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.shutdown();
}

/// Publishes `RealtimeUserFanoutEvent` for the connection's user when the client
/// sends `fanout`.
struct FanoutHandler;

#[async_trait::async_trait]
impl IWebSocketHandler for FanoutHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        let user_id = connection
            .context()
            .identity()
            .map(|identity| identity.id.clone())
            .ok_or_else(|| Error::unauthorized("login required"))?;
        // `add_mediator()` registers the mediator; the connection scope resolves
        // it, so the handler does not need a pre-built provider.
        let mediator = connection
            .context()
            .resolve::<webx::Mediator>()
            .ok_or_else(|| Error::internal("mediator is not registered".to_string()))?;
        while let Some(message) = connection.recv().await {
            if let RealtimeMessage::Text(text) = message? {
                if text == "fanout" {
                    mediator
                        .publish(RealtimeUserFanoutEvent::to_user(
                            user_id.clone(),
                            "fanout-payload",
                        ))
                        .await?;
                } else {
                    connection.send_text(text).await?;
                }
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn user_fanout_reaches_every_local_connection_of_a_user() {
    let port = free_port();
    let handle = spawn_host_with(port, Arc::new(FanoutHandler), |builder| {
        builder
            .use_authentication()
            .configure(|app| {
                app.options(|options| options.jwt.secret = test_secret().to_string());
            })
            .register(|services| services.add_mediator())
    })
    .await;

    let first = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue first");
    let second = issuer()
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue second");
    let (mut first_client, _) =
        connect_with_header(port, "authorization", format!("Bearer {}", first.token))
            .await
            .expect("first handshake");
    let (mut second_client, _) =
        connect_with_header(port, "authorization", format!("Bearer {}", second.token))
            .await
            .expect("second handshake");

    first_client
        .send(Message::Text("fanout".into()))
        .await
        .expect("send trigger");
    for client in [&mut first_client, &mut second_client] {
        let frame = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("fanout deadline")
            .expect("fanout frame")
            .expect("fanout payload");
        assert_eq!(frame, Message::Text("fanout-payload".into()));
    }
    handle.shutdown();
}
