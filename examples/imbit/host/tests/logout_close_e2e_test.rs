//! 2.9.2 end-to-end: logout revokes the JWT that authorized a live WebSocket.
//!
//! The host re-checks the upgrade credential (`WebSocketLimits::
//! with_session_revalidation`, 500 ms demo default): `POST /api/logout` revokes
//! the token's `jti` in the shared session store, and the live socket is closed
//! with `1008` / `SessionRevoked` within one re-check interval. Other users'
//! sessions are unaffected.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

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

async fn connect_ws(port: u16, token: &str) -> Client {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    let mut request = format!("ws://127.0.0.1:{port}/ws/im")
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("auth header"),
    );
    let (client, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws handshake");
    assert_eq!(response.status(), 101);
    client
}

async fn send_json(client: &mut Client, value: serde_json::Value) {
    client
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("ws send");
}

async fn recv_type(client: &mut Client, wanted: &str) -> serde_json::Value {
    for _ in 0..10 {
        let message = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("ws frame deadline")
            .expect("ws frame")
            .expect("ws frame ok");
        let Message::Text(text) = message else {
            panic!("expected text frame, got {message:?}");
        };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("ws json");
        if frame["type"] == wanted {
            return frame;
        }
        assert!(
            frame.get("error").is_none(),
            "server error frame while waiting for `{wanted}`: {frame}"
        );
    }
    panic!("no `{wanted}` frame arrived");
}

/// Wait for the framework close frame; assert the revocation code and reason.
async fn await_policy_violation_close(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, client.next())
            .await
            .expect("close frame deadline")
            .expect("session ended without a close frame")
            .expect("wire error");
        if let Message::Close(Some(frame)) = frame {
            assert_eq!(
                u16::from(frame.code),
                1008,
                "revocation must use policy-violation close"
            );
            assert_eq!(frame.reason.to_string(), "session revoked");
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logout_closes_the_live_websocket_with_1008() {
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
    let bob = login(&http, &base, "bob").await;
    let alice_token = alice["token"].as_str().expect("alice token").to_string();
    let bob_token = bob["token"].as_str().expect("bob token").to_string();

    let room: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "name": "revocation" }))
        .send()
        .await
        .expect("create room")
        .json()
        .await
        .expect("room body");
    let room_id = room["id"].as_str().expect("room id").to_string();
    http.post(format!("{base}/api/rooms/{room_id}/join"))
        .bearer_auth(&bob_token)
        .send()
        .await
        .expect("bob join");

    let mut alice_ws = connect_ws(port, &alice_token).await;
    let mut bob_ws = connect_ws(port, &bob_token).await;
    for client in [&mut alice_ws, &mut bob_ws] {
        send_json(
            client,
            serde_json::json!({ "type": "join", "room_id": room_id }),
        )
        .await;
        let _ = recv_type(client, "joined").await;
    }

    // Logout revokes the `jti` bound from the verified bearer token.
    let logout = http
        .post(format!("{base}/api/logout"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("logout request");
    assert!(logout.status().is_success(), "logout failed");

    // The revalidation interval (500 ms demo default) closes the live socket.
    await_policy_violation_close(&mut alice_ws).await;

    // Another user's session is not affected: bob still talks on his socket.
    send_json(
        &mut bob_ws,
        serde_json::json!({
            "type": "send", "room_id": room_id,
            "client_msg_id": "still-alive", "text": "bob-is-fine"
        }),
    )
    .await;
    let ack = recv_type(&mut bob_ws, "ack").await;
    assert_eq!(ack["duplicate"], false);

    bob_ws.close(None).await.expect("close bob");
    handle.shutdown();
}
