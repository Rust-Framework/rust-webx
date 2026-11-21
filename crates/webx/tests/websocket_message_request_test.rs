//! 2.3-3 message-as-request end-to-end test.
//!
//! A WebSocket text frame is a request: JSON is decoded into `Req`, dispatched
//! through the **same** `#[handler] IRequestHandler` path HTTP uses (per-message
//! DI scope created by the mediator), and `Rsp` is encoded back as a JSON frame.
//! Errors become structured `{"error":{...}}` frames.

use std::sync::atomic::{AtomicU32, Ordering};
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

/// Per-message scoped service: a fresh instance in every message scope.
#[derive(Default)]
struct MessageScopeProbe {
    id: u32,
}

static PROBE_NEXT: AtomicU32 = AtomicU32::new(1);

impl MessageScopeProbe {
    fn new() -> Self {
        Self {
            id: PROBE_NEXT.fetch_add(1, Ordering::SeqCst),
        }
    }
}

/// Connection-scoped service: one instance for the whole WebSocket connection.
#[derive(Default)]
struct ConnectionCount(AtomicU32);

#[derive(serde::Deserialize)]
struct EchoRequest {
    text: String,
}

#[derive(serde::Serialize)]
struct EchoReply {
    text: String,
    /// Fresh per message (per-message DI scope).
    probe_id: u32,
    /// Increments across messages of one connection (connection DI scope).
    connection_count: u32,
    /// Authenticated operator (upgrade-request credentials), if any.
    operator: Option<String>,
}

impl IRequest<EchoReply> for EchoRequest {}

#[derive(Inject)]
#[allow(dead_code)]
struct EchoMessageHandler {
    #[inject]
    probe: Arc<MessageScopeProbe>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<EchoRequest, EchoReply> for EchoMessageHandler {
    async fn handle(&mut self, req: EchoRequest) -> ActionResult<EchoReply> {
        if req.text.is_empty() {
            return Err(Error::Validation("text is required".into()));
        }
        // The connection stays reachable from the message handler even though
        // its own `#[inject]` dependencies come from the per-message scope.
        let context = RequestContext::connection().expect("connection context in scope");
        let counter = context
            .resolve::<ConnectionCount>()
            .expect("connection-scoped service");
        let connection_count = counter.0.fetch_add(1, Ordering::SeqCst) + 1;
        // The upgrade-request identity must be visible from both the message
        // context and the connection context.
        let operator = RequestContext::operator_id();
        assert_eq!(
            operator,
            context.identity().map(|user| user.id.clone()),
            "message identity must match the connection identity"
        );
        Ok(EchoReply {
            text: req.text,
            probe_id: self.probe.id,
            connection_count,
            operator,
        })
    }
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn read_json(client: &mut Client) -> serde_json::Value {
    let message = client.next().await.expect("frame").expect("frame ok");
    let text = match message {
        Message::Text(text) => text.to_string(),
        other => panic!("expected a text frame, got {other:?}"),
    };
    serde_json::from_str(&text).expect("json frame")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_frame_is_a_request_dispatched_through_the_same_handler_path() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| {
            svc.add_mediator()
                .scoped(|_| Arc::new(MessageScopeProbe::new()))
                .scoped(|_| Arc::new(ConnectionCount::default()))
        })
        .use_websocket_request::<EchoRequest, EchoReply>("/rpc")
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let (mut client, response) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc"))
            .await
            .expect("handshake");
    assert_eq!(response.status(), 101);

    client
        .send(Message::Text(r#"{"text":"hello"}"#.into()))
        .await
        .expect("send first");
    let first = read_json(&mut client).await;
    assert_eq!(first["text"], "hello");
    assert_eq!(first["connection_count"], 1);
    let first_probe = first["probe_id"].as_u64().expect("probe id");

    client
        .send(Message::Text(r#"{"text":"again"}"#.into()))
        .await
        .expect("send second");
    let second = read_json(&mut client).await;
    assert_eq!(second["text"], "again");
    // Connection scope: the counter is the same instance across messages.
    assert_eq!(second["connection_count"], 2);
    // Message scope: the handler's injected service is rebuilt per message.
    let second_probe = second["probe_id"].as_u64().expect("probe id");
    assert!(
        second_probe > first_probe,
        "each message must get a fresh per-message scope ({first_probe} -> {second_probe})"
    );

    // Business errors use the same Error contract as HTTP, as a structured frame.
    client
        .send(Message::Text(r#"{"text":""}"#.into()))
        .await
        .expect("send invalid");
    let failure = read_json(&mut client).await;
    assert_eq!(failure["error"]["code"], "validation_failed");
    assert!(failure["error"]["message"].is_string());

    // Malformed JSON is a protocol-level error, not a dropped connection.
    client
        .send(Message::Text("not-json".into()))
        .await
        .expect("send garbage");
    let failure = read_json(&mut client).await;
    assert_eq!(failure["error"]["code"], "invalid_message");

    // Frame binding errors carry the JSON field path (HTTP-400 semantics).
    client
        .send(Message::Text(r#"{"text":123}"#.into()))
        .await
        .expect("send wrong type");
    let failure = read_json(&mut client).await;
    assert_eq!(failure["error"]["code"], "validation_failed");
    assert_eq!(failure["error"]["field"], "text");

    client
        .send(Message::Text("{}".into()))
        .await
        .expect("send missing field");
    let failure = read_json(&mut client).await;
    assert_eq!(failure["error"]["code"], "validation_failed");
    // A missing field has no path segment in serde's error; the message names it.
    assert!(failure["error"]["field"].is_null());
    assert!(failure["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("missing field")));

    // The session survived all of the above and still serves requests.
    client
        .send(Message::Text(r#"{"text":"last"}"#.into()))
        .await
        .expect("send last");
    let last = read_json(&mut client).await;
    assert_eq!(last["text"], "last");
    assert_eq!(last["connection_count"], 3);

    client.close(None).await.expect("close");
    handle.shutdown();
}

/// Minimal authentication scheme for the upgrade-request identity test.
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
        Ok(Some(Box::new(ApiKeyClaims::new(user).with_tenant("acme"))))
    }
}

async fn spawn_rpc_host(port: u16, authenticate: bool) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let mut builder = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| {
            svc.add_mediator()
                .scoped(|_| Arc::new(MessageScopeProbe::new()))
                .scoped(|_| Arc::new(ConnectionCount::default()))
        })
        .use_websocket_request::<EchoRequest, EchoReply>("/rpc");
    if authenticate {
        builder = builder.add_authentication_scheme(Arc::new(TestUserScheme));
    }
    let host = builder.build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    handle
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrade_request_identity_flows_into_message_handlers() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    let port = free_port();
    let handle = spawn_rpc_host(port, true).await;

    // Authenticated upgrade: the scheme runs in the normal HTTP pipeline before
    // the WebSocket middleware, so the session carries the identity.
    let mut request = format!("ws://127.0.0.1:{port}/rpc")
        .into_client_request()
        .expect("client request");
    request
        .headers_mut()
        .insert("x-test-user", HeaderValue::from_static("alice"));
    let (mut client, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("authenticated handshake");
    client
        .send(Message::Text(r#"{"text":"whoami"}"#.into()))
        .await
        .expect("send");
    let reply = read_json(&mut client).await;
    assert_eq!(reply["operator"], "alice");

    // Anonymous upgrade: same endpoint, no credentials -> no identity.
    let (mut anonymous, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc"))
        .await
        .expect("anonymous handshake");
    anonymous
        .send(Message::Text(r#"{"text":"whoami"}"#.into()))
        .await
        .expect("send");
    let reply = read_json(&mut anonymous).await;
    assert!(reply["operator"].is_null());

    client.close(None).await.expect("close");
    anonymous.close(None).await.expect("close");
    handle.shutdown();
}
