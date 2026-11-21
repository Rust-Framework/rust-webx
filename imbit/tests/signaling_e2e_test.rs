//! 2.4-1 end-to-end: WebRTC signaling through the IM reference app + TURN
//! REST credentials.
//!
//! Signaling rides the same message-as-request WebSocket channel as chat:
//! targeted SDP offer/answer between two members and room broadcast for ICE
//! candidates; non-members are rejected. TURN credentials are issued over HTTP
//! (coturn REST API) for the authenticated caller.

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
    assert!(response.status().is_success(), "{:?}", response.status());
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
async fn signaling_and_turn_credentials_work_end_to_end() {
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
    let alice = login(&http, &base, "alice").await;
    let bob = login(&http, &base, "bob").await;
    let charlie = login(&http, &base, "charlie").await;
    let alice_token = alice["token"].as_str().expect("alice token").to_string();
    let bob_token = bob["token"].as_str().expect("bob token").to_string();
    let charlie_token = charlie["token"]
        .as_str()
        .expect("charlie token")
        .to_string();
    let alice_id = alice["user_id"].as_str().expect("alice id").to_string();
    let bob_id = bob["user_id"].as_str().expect("bob id").to_string();

    let room: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "name": "call" }))
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
    let mut charlie_ws = connect_ws(port, &charlie_token).await;
    for client in [&mut alice_ws, &mut bob_ws] {
        send_json(
            client,
            serde_json::json!({ "type": "join", "room_id": room_id }),
        )
        .await;
        let _ = recv_type(client, "joined").await;
    }

    // 1:1 negotiation: alice -> bob offer.
    send_json(
        &mut alice_ws,
        serde_json::json!({
            "type": "signal", "room_id": room_id, "to": "bob",
            "kind": "offer", "payload": { "sdp": "v=0", "type": "offer" }
        }),
    )
    .await;
    let ack = recv_type(&mut alice_ws, "signaled").await;
    assert_eq!(ack["delivered"], 1);
    let offer = recv_type(&mut bob_ws, "signal").await;
    let message = &offer["message"];
    assert_eq!(message["kind"], "offer");
    assert_eq!(message["from"], "alice");
    assert_eq!(message["to"], bob_id.as_str());
    assert_eq!(message["payload"]["sdp"], "v=0");

    // Answer back to alice.
    send_json(
        &mut bob_ws,
        serde_json::json!({
            "type": "signal", "room_id": room_id, "to": "alice",
            "kind": "answer", "payload": { "sdp": "v=0", "type": "answer" }
        }),
    )
    .await;
    let ack = recv_type(&mut bob_ws, "signaled").await;
    assert_eq!(ack["delivered"], 1);
    let answer = recv_type(&mut alice_ws, "signal").await;
    assert_eq!(answer["message"]["kind"], "answer");
    assert_eq!(answer["message"]["from"], "bob");
    assert_eq!(answer["message"]["to"], alice_id.as_str());

    // Room broadcast (trickle ICE candidate) reaches other members only.
    send_json(
        &mut bob_ws,
        serde_json::json!({
            "type": "signal", "room_id": room_id,
            "kind": "ice_candidate", "payload": { "candidate": "candidate:1 1 udp" }
        }),
    )
    .await;
    let ack = recv_type(&mut bob_ws, "signaled").await;
    assert_eq!(ack["delivered"], 1, "broadcast excludes the sender");
    let candidate = recv_type(&mut alice_ws, "signal").await;
    assert_eq!(candidate["message"]["kind"], "ice_candidate");
    assert_eq!(
        candidate["message"]["payload"]["candidate"],
        "candidate:1 1 udp"
    );

    // Non-members cannot signal.
    send_json(
        &mut charlie_ws,
        serde_json::json!({
            "type": "signal", "room_id": room_id, "to": "bob",
            "kind": "offer", "payload": { "sdp": "v=0" }
        }),
    )
    .await;
    let failure = recv_frame(&mut charlie_ws).await;
    assert_eq!(failure["error"]["code"], "forbidden", "frame: {failure}");

    // Unknown target is a typed not-found, not a silent drop.
    send_json(
        &mut alice_ws,
        serde_json::json!({
            "type": "signal", "room_id": room_id, "to": "nobody",
            "kind": "offer", "payload": { "sdp": "v=0" }
        }),
    )
    .await;
    let failure = recv_frame(&mut alice_ws).await;
    assert_eq!(failure["error"]["code"], "not_found", "frame: {failure}");

    // TURN REST credentials for the authenticated caller.
    let turn: serde_json::Value = http
        .get(format!("{base}/api/turn"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("turn request")
        .json()
        .await
        .expect("turn body");
    let username = turn["username"].as_str().expect("turn username");
    assert!(
        username.ends_with(&format!(":{alice_id}")),
        "username must end with the user id: {username}"
    );
    assert!(
        username
            .split(':')
            .next()
            .is_some_and(|expiry| expiry.parse::<u64>().is_ok_and(|expiry| expiry > 0)),
        "username must start with a unix expiry: {username}"
    );
    assert!(!turn["credential"].as_str().expect("credential").is_empty());
    assert_eq!(turn["ttl_seconds"], webx::DEFAULT_TURN_TTL_SECONDS);
    let urls = turn["urls"].as_array().expect("urls");
    assert!(!urls.is_empty());

    // TTL override is clamped to the supported range.
    let short: serde_json::Value = http
        .get(format!("{base}/api/turn?ttl_seconds=60"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .expect("turn request")
        .json()
        .await
        .expect("turn body");
    assert_eq!(short["ttl_seconds"], 60);

    alice_ws.close(None).await.expect("close alice");
    bob_ws.close(None).await.expect("close bob");
    charlie_ws.close(None).await.expect("close charlie");
    handle.shutdown();
}
