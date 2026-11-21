//! End-to-end IM reference test (2.3-7).
//!
//! Real HTTP + real WebSocket clients against the reference host:
//! login (framework token ports) -> create/join room -> join over WS ->
//! send/broadcast -> idempotent retry -> reconnect + offline catch-up ->
//! presence.

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

async fn login(
    http: &reqwest::Client,
    base: &str,
    username: &str,
    password: &str,
) -> serde_json::Value {
    let response = http
        .post(format!("{base}/api/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
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

async fn recv_frame(client: &mut Client) -> serde_json::Value {
    let message = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("ws frame deadline")
        .expect("ws frame")
        .expect("ws frame ok");
    match message {
        Message::Text(text) => serde_json::from_str(&text).expect("ws json"),
        other => panic!("expected text frame, got {other:?}"),
    }
}

/// Read frames until one has `type == wanted`.
async fn recv_type(client: &mut Client, wanted: &str) -> serde_json::Value {
    for _ in 0..10 {
        let frame = recv_frame(client).await;
        if frame["type"] == wanted {
            return frame;
        }
        if frame.get("error").is_some() {
            panic!("server error frame while waiting for `{wanted}`: {frame}");
        }
    }
    panic!("no `{wanted}` frame arrived");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn im_reference_flow_login_rooms_messages_reconnect_catch_up() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let address = format!("127.0.0.1:{port}");
    let host = imbit::build_host();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let http = reqwest::Client::new();
    let alice_login = login(&http, &base, "alice", "secret-a").await;
    let bob_login = login(&http, &base, "bob", "secret-b").await;
    let alice_token = alice_login["token"]
        .as_str()
        .expect("alice token")
        .to_string();
    let bob_token = bob_login["token"].as_str().expect("bob token").to_string();
    assert_eq!(alice_login["username"], "alice");

    // Login is idempotent and verifies the password (wrong password is 401).
    let wrong = http
        .post(format!("{base}/api/login"))
        .json(&serde_json::json!({ "username": "alice", "password": "wrong" }))
        .send()
        .await
        .expect("wrong login request");
    assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Create the room over HTTP (Bearer token) and join with bob.
    let room: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "name": "general" }))
        .send()
        .await
        .expect("create room")
        .json()
        .await
        .expect("room body");
    let room_id = room["id"].as_str().expect("room id").to_string();
    let joined: serde_json::Value = http
        .post(format!("{base}/api/rooms/{room_id}/join"))
        .bearer_auth(&bob_token)
        .send()
        .await
        .expect("join room")
        .json()
        .await
        .expect("join body");
    assert_eq!(joined["members"].as_array().expect("members").len(), 2);

    // Both clients connect over WebSocket (JWT from the login endpoint).
    let mut alice_ws = connect_ws(port, &alice_token).await;
    let mut bob_ws = connect_ws(port, &bob_token).await;
    for (client, room_id) in [(&mut alice_ws, &room_id), (&mut bob_ws, &room_id)] {
        send_json(
            client,
            serde_json::json!({ "type": "join", "room_id": room_id }),
        )
        .await;
        let joined = recv_type(client, "joined").await;
        assert_eq!(joined["room_id"], room_id.as_str());
    }

    // Send: ack with seq 1, and the room receives the broadcast event.
    send_json(
        &mut alice_ws,
        serde_json::json!({
            "type": "send", "room_id": room_id,
            "client_msg_id": "m1", "text": "hello"
        }),
    )
    .await;
    let ack = recv_type(&mut alice_ws, "ack").await;
    assert_eq!(ack["seq"], 1);
    assert_eq!(ack["duplicate"], false);
    let event = recv_type(&mut bob_ws, "message").await;
    assert_eq!(event["room_id"], room_id.as_str());
    assert_eq!(event["seq"], 1);
    assert_eq!(event["from"], "alice");
    assert_eq!(event["text"], "hello");

    // Retry with the same client message id: idempotent ack, no duplicate event.
    send_json(
        &mut alice_ws,
        serde_json::json!({
            "type": "send", "room_id": room_id,
            "client_msg_id": "m1", "text": "hello"
        }),
    )
    .await;
    let duplicate = recv_type(&mut alice_ws, "ack").await;
    assert_eq!(duplicate["seq"], 1);
    assert_eq!(duplicate["duplicate"], true);
    let unexpected = tokio::time::timeout(Duration::from_millis(300), bob_ws.next()).await;
    assert!(
        unexpected.is_err(),
        "idempotent retry must not be broadcast twice"
    );

    // Bob sends while alice is offline; bob receives its own broadcast event and
    // the ack in either order (broadcast happens before the reply is queued).
    send_json(
        &mut bob_ws,
        serde_json::json!({
            "type": "send", "room_id": room_id,
            "client_msg_id": "m2", "text": "while-you-were-away"
        }),
    )
    .await;
    let mut bob_ack = None;
    let mut bob_event = None;
    for _ in 0..2 {
        let frame = recv_frame(&mut bob_ws).await;
        match frame["type"].as_str() {
            Some("ack") => bob_ack = Some(frame),
            Some("message") => bob_event = Some(frame),
            other => panic!("unexpected bob frame type: {other:?}"),
        }
    }
    let bob_ack = bob_ack.expect("bob ack");
    assert_eq!(bob_ack["seq"], 2);
    assert_eq!(bob_event.expect("bob broadcast")["seq"], 2);
    alice_ws.close(None).await.expect("alice close");

    let mut alice_reconnected = connect_ws(port, &alice_token).await;
    send_json(
        &mut alice_reconnected,
        serde_json::json!({ "type": "join", "room_id": room_id }),
    )
    .await;
    let _ = recv_type(&mut alice_reconnected, "joined").await;
    send_json(
        &mut alice_reconnected,
        serde_json::json!({ "type": "history", "room_id": room_id, "after_seq": 1 }),
    )
    .await;
    let history = recv_type(&mut alice_reconnected, "history").await;
    let messages = history["messages"].as_array().expect("history messages");
    assert_eq!(messages.len(), 1, "exactly the missed message");
    assert_eq!(messages[0]["seq"], 2);
    assert_eq!(messages[0]["text"], "while-you-were-away");

    // Presence: both members online in the room.
    send_json(
        &mut alice_reconnected,
        serde_json::json!({ "type": "presence", "room_id": room_id }),
    )
    .await;
    let presence = recv_type(&mut alice_reconnected, "presence").await;
    let online = presence["online"].as_array().expect("online list");
    let online: Vec<&str> = online.iter().filter_map(|name| name.as_str()).collect();
    assert!(online.contains(&"alice"), "online: {online:?}");
    assert!(online.contains(&"bob"), "online: {online:?}");

    // HTTP catch-up mirrors the WebSocket history (offline clients).
    let catch_up: serde_json::Value = http
        .get(format!("{base}/api/rooms/{room_id}/messages?after_seq=1"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("history request")
        .json()
        .await
        .expect("history body");
    assert_eq!(catch_up["messages"].as_array().expect("messages").len(), 1);

    alice_reconnected.close(None).await.expect("close alice");
    bob_ws.close(None).await.expect("close bob");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_ws_gets_a_structured_error_frame() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = imbit::build_host();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    let request = format!("ws://127.0.0.1:{port}/ws/im")
        .into_client_request()
        .expect("request");
    let (mut client, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("handshake");
    assert_eq!(response.status(), 101);
    send_json(
        &mut client,
        serde_json::json!({ "type": "join", "room_id": "r1" }),
    )
    .await;
    let frame = recv_frame(&mut client).await;
    assert_eq!(
        frame["error"]["code"], "unauthorized",
        "anonymous join must be rejected: {frame}"
    );
    client.close(None).await.ok();
    handle.shutdown();
}
