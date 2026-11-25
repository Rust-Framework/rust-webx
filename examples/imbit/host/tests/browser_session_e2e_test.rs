//! 2.9.2 end-to-end: browser access to the IM reference app.
//!
//! A bearer client exchanges its token for two cookies (`HttpOnly` session +
//! readable CSRF), upgrades `/ws/im` with the cookie only, and chats. Unsafe
//! cookie-authenticated requests are CSRF-checked; bearer clients are not.
//!
//! Origin checking is a separate test binary (`websocket_origin_e2e_test.rs`)
//! because it is configured through a process-wide environment variable.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

const SESSION_COOKIE_PREFIX: &str = "__Host-webx_session=";
const CSRF_COOKIE_PREFIX: &str = "__Host-webx_csrf=";

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

/// `name=value` pair of a `Set-Cookie` header (attributes dropped).
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .expect("a cookie has a name=value pair")
        .trim()
        .to_string()
}

/// `POST /api/session`: bearer token -> `(session cookie pair, csrf cookie pair)`.
async fn exchange_session(http: &reqwest::Client, base: &str, token: &str) -> (String, String) {
    let response = http
        .post(format!("{base}/api/session"))
        .bearer_auth(token)
        .send()
        .await
        .expect("session exchange");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "session exchange failed"
    );
    let set_cookies: Vec<String> = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("set-cookie is ascii").to_string())
        .collect();
    let body: serde_json::Value = response.json().await.expect("session body");

    let session = set_cookies
        .iter()
        .find(|cookie| cookie.starts_with(SESSION_COOKIE_PREFIX))
        .expect("a session cookie")
        .clone();
    let csrf = set_cookies
        .iter()
        .find(|cookie| cookie.starts_with(CSRF_COOKIE_PREFIX))
        .expect("a csrf cookie")
        .clone();

    // The session id must be unreadable from JavaScript; the CSRF token must be
    // readable (the app copies it into the request header).
    assert!(session.contains("HttpOnly"), "session cookie: {session}");
    assert!(session.contains("Secure"), "session cookie: {session}");
    assert!(
        session.contains("SameSite=Lax"),
        "session cookie: {session}"
    );
    assert!(!csrf.contains("HttpOnly"), "csrf cookie: {csrf}");
    assert!(body["user_id"].is_string(), "session body: {body}");

    (cookie_pair(&session), cookie_pair(&csrf))
}

fn csrf_value(csrf_pair: &str) -> String {
    csrf_pair
        .split_once('=')
        .expect("csrf cookie has a value")
        .1
        .to_string()
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn connect_ws_with_cookie(port: u16, cookies: &str) -> Client {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    let mut request = format!("ws://127.0.0.1:{port}/ws/im")
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        "cookie",
        HeaderValue::from_str(cookies).expect("valid cookie header"),
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
async fn cookie_session_upgrades_the_websocket_and_chats() {
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
    let alice = login(&http, &base, "alice", "secret-a").await;
    let bob = login(&http, &base, "bob", "secret-b").await;
    let alice_token = alice["token"].as_str().expect("alice token").to_string();
    let bob_token = bob["token"].as_str().expect("bob token").to_string();
    let alice_id = alice["user_id"].as_str().expect("alice id").to_string();

    // A bearer client creates the room (bearer path is unaffected by cookies).
    let room: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "name": "browser" }))
        .send()
        .await
        .expect("create room")
        .json()
        .await
        .expect("room body");
    let room_id = room["id"].as_str().expect("room id").to_string();

    // Exchange both tokens for cookie credentials.
    let (alice_session, alice_csrf) = exchange_session(&http, &base, &alice_token).await;
    let (bob_session, bob_csrf) = exchange_session(&http, &base, &bob_token).await;
    let alice_cookies = format!("{alice_session}; {alice_csrf}");
    let bob_cookies = format!("{bob_session}; {bob_csrf}");

    // Browsers upgrade with cookies only: no Authorization header.
    let mut alice_ws = connect_ws_with_cookie(port, &alice_cookies).await;
    let mut bob_ws = connect_ws_with_cookie(port, &bob_cookies).await;
    for client in [&mut alice_ws, &mut bob_ws] {
        send_json(
            client,
            serde_json::json!({ "type": "join", "room_id": room_id }),
        )
        .await;
        let joined = recv_type(client, "joined").await;
        assert_eq!(joined["room_id"], room_id.as_str());
    }

    // Send/receive over the cookie-authenticated session.
    send_json(
        &mut alice_ws,
        serde_json::json!({
            "type": "send", "room_id": room_id,
            "client_msg_id": "browser-1", "text": "hello-from-cookie"
        }),
    )
    .await;
    let ack = recv_type(&mut alice_ws, "ack").await;
    assert_eq!(ack["seq"], 1);
    let event = recv_type(&mut bob_ws, "message").await;
    assert_eq!(event["from"], "alice");
    assert_eq!(event["text"], "hello-from-cookie");

    // Cookie-authenticated unsafe request without the CSRF header is rejected.
    let missing_csrf = http
        .post(format!("{base}/api/rooms"))
        .header(reqwest::header::COOKIE, alice_cookies.as_str())
        .json(&serde_json::json!({ "name": "no-csrf" }))
        .send()
        .await
        .expect("no-csrf request");
    assert_eq!(missing_csrf.status(), reqwest::StatusCode::FORBIDDEN);

    // ... and so is a mismatched token.
    let wrong_csrf = http
        .post(format!("{base}/api/rooms"))
        .header(reqwest::header::COOKIE, alice_cookies.as_str())
        .header("x-csrf-token", "not-the-token")
        .json(&serde_json::json!({ "name": "wrong-csrf" }))
        .send()
        .await
        .expect("wrong-csrf request");
    assert_eq!(wrong_csrf.status(), reqwest::StatusCode::FORBIDDEN);

    // The echoed token passes, and the request runs under the cookie identity.
    let with_csrf: serde_json::Value = http
        .post(format!("{base}/api/rooms"))
        .header(reqwest::header::COOKIE, alice_cookies.as_str())
        .header("x-csrf-token", csrf_value(&alice_csrf))
        .json(&serde_json::json!({ "name": "with-csrf" }))
        .send()
        .await
        .expect("with-csrf request")
        .json()
        .await
        .expect("with-csrf body");
    assert_eq!(
        with_csrf["owner_id"], alice_id,
        "the session subject must be the bearer subject"
    );

    // Bearer requests carry no session cookie and are not CSRF-checked.
    let bearer_room = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&alice_token)
        .json(&serde_json::json!({ "name": "bearer" }))
        .send()
        .await
        .expect("bearer create");
    assert_eq!(bearer_room.status(), reqwest::StatusCode::OK);

    alice_ws.close(None).await.expect("close alice");
    bob_ws.close(None).await.expect("close bob");
    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_session_exchange_is_unauthorized() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let address = format!("127.0.0.1:{port}");
    let host = imbit_host::build_host();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/session"))
        .send()
        .await
        .expect("anonymous session exchange");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    handle.shutdown();
}
