//! 2.9.2 end-to-end: origin allow-list on the WebSocket handshake.
//!
//! The allow-list is read from `IMBIT_ORIGINS` when the host is built, which is
//! process-wide: this test lives in its own binary so it cannot disturb the
//! bearer/cookie tests (a missing `Origin` must keep working by default).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const ALLOWED_ORIGIN: &str = "https://app.example.com";

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("imbit did not become ready");
}

async fn login(http: &reqwest::Client, base: &str, username: &str) -> serde_json::Value {
    let response = http
        .post(format!("{base}/api/login"))
        .json(&serde_json::json!({ "username": username, "password": "secret-123" }))
        .send()
        .await
        .expect("login request");
    assert!(
        response.status().is_success(),
        "login failed: {:?}",
        response.status()
    );
    response.json().await.expect("login body")
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
type Handshake = std::result::Result<
    (
        Client,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
>;

/// Upgrade `/ws/im` with an optional browser `Origin` header and a bearer token.
async fn handshake(port: u16, token: &str, origin: Option<&str>) -> Handshake {
    let mut request = format!("ws://127.0.0.1:{port}/ws/im")
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("auth header"),
    );
    if let Some(origin) = origin {
        request.headers_mut().insert(
            "origin",
            HeaderValue::from_str(origin).expect("origin header"),
        );
    }
    tokio_tungstenite::connect_async(request).await
}

fn expect_forbidden(handshake: Handshake) {
    match handshake {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(
                response.status(),
                tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN,
                "a disallowed Origin must fail the handshake before the upgrade"
            );
        }
        Ok(_) => panic!("handshake with a disallowed Origin must be rejected"),
        Err(error) => panic!("unexpected handshake error: {error}"),
    }
}

async fn connect_room(port: u16, token: &str, room_id: &str) -> Client {
    let (mut client, response) = handshake(port, token, Some(ALLOWED_ORIGIN))
        .await
        .expect("allowed Origin handshake");
    assert_eq!(response.status(), 101);
    client
        .send(Message::Text(
            serde_json::json!({ "type": "join", "room_id": room_id })
                .to_string()
                .into(),
        ))
        .await
        .expect("ws send");
    let message = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("joined deadline")
        .expect("joined frame")
        .expect("joined ok");
    match message {
        Message::Text(text) => {
            let frame: serde_json::Value = serde_json::from_str(&text).expect("joined json");
            assert_eq!(frame["type"], "joined");
        }
        other => panic!("expected text frame, got {other:?}"),
    }
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn imbit_origins_whitelists_the_websocket_handshake() {
    // Configured before the host is built: the allow-list is builder state.
    std::env::set_var(
        "IMBIT_ORIGINS",
        format!("{ALLOWED_ORIGIN}, https://admin.example.com"),
    );

    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let address = format!("127.0.0.1:{port}");
    let host = imbit_host::build_host();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let http = reqwest::Client::new();
    let alice = login(&http, &base, "alice").await;
    let token = alice["token"].as_str().expect("token").to_string();
    // HTTP is not origin-restricted (only the WebSocket handshake carries
    // automatically attached browser credentials).
    let room: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "name": "origins" }))
        .send()
        .await
        .expect("create room")
        .json()
        .await
        .expect("room body");
    let room_id = room["id"].as_str().expect("room id").to_string();

    // A browser always sends Origin; a missing one is rejected while the list
    // is configured (non-browser clients must then opt in explicitly).
    expect_forbidden(handshake(port, &token, None).await);
    // A cross-site Origin is rejected before the upgrade.
    expect_forbidden(handshake(port, &token, Some("https://evil.example.com")).await);
    // The allow-listed Origin upgrades and the session works.
    let mut client = connect_room(port, &token, &room_id).await;

    std::env::remove_var("IMBIT_ORIGINS");
    client.close(None).await.expect("close client");
    handle.shutdown();

    // Without `IMBIT_ORIGINS` the handshake Origin is not checked, so the same
    // (wrong) Origin now upgrades: existing bearer/CLI clients are unaffected.
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = imbit_host::build_host();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    let (_client, response) = handshake(port, &token, Some("https://evil.example.com"))
        .await
        .expect("unset IMBIT_ORIGINS must not check the Origin");
    assert_eq!(response.status(), 101);
    handle.shutdown();
}
