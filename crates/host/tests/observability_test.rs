//! Observability contract tests: the request span tree and the long-connection
//! lifecycle events defined in `docs/design/observability-contract.md`.
//!
//! The tests install a real `tracing-subscriber` layer that records every span
//! (name, parent name, fields — including fields recorded after creation) and
//! every event, then drive the framework's actual execution paths:
//!
//! * the engine pipeline with the tracing + authentication middleware and the
//!   real `StubEndpoint` adapter (root span, auth, routing, handler, binding);
//! * an SSE response through `EngineContext::write_endpoint_result` and
//!   `finalize` (connection.opened / connection.closed);
//! * a WebSocket upgrade against a real `Host` (connection.opened / closed).
//!
//! Assertions filter by the unique trace id / path of each scenario because the
//! tests share one process-wide subscriber.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use webx_host::MiddlewareSpec;

use bytes::Bytes;
use tokio::net::TcpStream;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

use webx_core::action::{EndpointResult, JsonResult, Response};
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::Result as WebxResult;
use webx_core::http::IHttpContext;
use webx_core::mode::AppMode;
use webx_core::realtime::{IConnection, IWebSocketHandler, RealtimeMessage};
use webx_core::routing::IEndpoint;
use webx_core::stream::{SseEvent, SseResult};
use webx_engine::{BodyLimits, EngineContext, FinalHandler, Pipeline};
use webx_host::auth::{ApiKeyClaims, AuthenticationMiddleware};
use webx_host::endpoint::StubEndpoint;
use webx_host::request_tracing::RequestTracing;
use webx_host::server::Host;

const REQUEST_TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const ORDERS_PATH: &str = "/api/obs/orders";
const SSE_TRACE_ID: &str = "11111111111111111111111111111111";
const SSE_PATH: &str = "/api/obs/events";
const WS_TRACE_ID: &str = "22222222222222222222222222222222";
const WS_PATH: &str = "/obs-ws";

// ---------------------------------------------------------------------------
// Capture subscriber
// ---------------------------------------------------------------------------

/// One recorded span: name, parent name (empty for a root span) and fields.
#[derive(Debug, Clone)]
struct CapturedSpan {
    name: String,
    parent: Option<String>,
    fields: HashMap<String, String>,
}

impl CapturedSpan {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// One recorded event with its fields (`message` included).
#[derive(Debug, Clone)]
struct CapturedEvent {
    fields: HashMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

#[derive(Default)]
struct Capture {
    spans: Mutex<Vec<CapturedSpan>>,
    span_ids: Mutex<HashMap<Id, usize>>,
    events: Mutex<Vec<CapturedEvent>>,
}

impl Capture {
    fn spans(&self) -> Vec<CapturedSpan> {
        self.spans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn events(&self) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The request root span of one scenario, selected by its path field.
    fn request_span(&self, path: &str) -> Option<CapturedSpan> {
        self.spans()
            .into_iter()
            .find(|span| span.name == "http.server.request" && span.field("path") == Some(path))
    }

    /// A child span by name and parent name.
    fn child_span(&self, name: &str, parent: &str) -> Option<CapturedSpan> {
        self.spans()
            .into_iter()
            .find(|span| span.name == name && span.parent.as_deref() == Some(parent))
    }

    /// Every event with the contract `event` field, in capture order.
    fn events_named(&self, event: &str) -> Vec<CapturedEvent> {
        self.events()
            .into_iter()
            .filter(|captured| captured.field("event") == Some(event))
            .collect()
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: HashMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

struct CaptureLayer(Arc<Capture>);

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);
        // Explicit `parent:` wins; otherwise the span is nested under the span
        // that was current when it was created.
        let parent = attrs
            .parent()
            .cloned()
            .or_else(|| ctx.current_span().id().cloned())
            .and_then(|parent| ctx.span(&parent).map(|span| span.name().to_string()));
        let index = {
            let mut spans = self
                .0
                .spans
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let index = spans.len();
            spans.push(CapturedSpan {
                name: attrs.metadata().name().to_string(),
                parent,
                fields: visitor.fields,
            });
            index
        };
        self.0
            .span_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), index);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor::default();
        values.record(&mut visitor);
        let index = self
            .0
            .span_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .copied();
        if let Some(index) = index {
            let mut spans = self
                .0
                .spans
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(span) = spans.get_mut(index) {
                span.fields.extend(visitor.fields);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        self.0
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(CapturedEvent {
                fields: visitor.fields,
            });
    }
}

/// Install the capturing subscriber once per test binary and share it.
fn capture() -> Arc<Capture> {
    static CAPTURE: OnceLock<Arc<Capture>> = OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = Arc::new(Capture::default());
            let subscriber =
                tracing_subscriber::registry().with(CaptureLayer(Arc::clone(&capture)));
            // A second call can only lose a race in exotic setups; the capture
            // is still the one the tests hold.
            let _ = tracing::subscriber::set_global_default(subscriber);
            capture
        })
        .clone()
}

/// Poll the capture until `find` returns `Some`, or panic after 5s.
async fn wait_for<T>(what: &str, mut find: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(found) = find() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// Scenario 1: request span tree + trace id correlation
// ---------------------------------------------------------------------------

/// A scheme that always authenticates, so the auth span takes the `ok` path.
struct AlwaysAuthenticated;

#[async_trait::async_trait]
impl IAuthenticationScheme for AlwaysAuthenticated {
    fn name(&self) -> &'static str {
        "test-scheme"
    }

    fn matches(&self, _ctx: &dyn IHttpContext) -> bool {
        true
    }

    async fn authenticate(
        &self,
        _ctx: &mut dyn IHttpContext,
    ) -> WebxResult<Option<Box<dyn IClaims>>> {
        Ok(Some(Box::new(ApiKeyClaims::new("user-1"))))
    }
}

/// Dispatch function of the recorded route: reads the body (the binding span)
/// and answers JSON through the normal action path.
fn orders_dispatch(
    ctx: &mut dyn IHttpContext,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = WebxResult<Response>> + Send + '_>> {
    Box::pin(async move {
        let _body = ctx.request_mut().body_bytes_shared().await?;
        Ok(Box::new(JsonResult::new(serde_json::json!({"ok": true}))) as Response)
    })
}

#[tokio::test]
async fn request_root_span_covers_the_layer_spans_and_matches_the_trace_id_header() {
    let capture = capture();

    let (parts, _) = http::Request::builder()
        .method("GET")
        .uri(ORDERS_PATH)
        .header("x-request-id", REQUEST_TRACE_ID)
        .body(())
        .expect("test request builds")
        .into_parts();
    let mut ctx = EngineContext::from_parts(
        parts,
        Bytes::new(),
        Arc::new(BodyLimits::default()),
        None,
        None,
    );

    let pipeline = Pipeline::new(vec![
        Arc::new(RequestTracing::new()),
        Arc::new(AuthenticationMiddleware::new(vec![Arc::new(
            AlwaysAuthenticated,
        )])),
    ]);

    let final_handler: FinalHandler = Arc::new(|ctx: &mut EngineContext| {
        Box::pin(async move {
            // The engine's router records the matched template here.
            ctx.set_route(Arc::from(ORDERS_PATH), Vec::new());
            let endpoint = StubEndpoint {
                method: "GET",
                path: ORDERS_PATH,
                handler_type: "observability_test::OrdersRequest",
                dispatch_fn: Some(orders_dispatch),
                authorization: &[],
                authorization_dynamic: false,
                authorizers: None,
                max_body_size: None,
            };
            endpoint.handle(ctx).await
        })
    });

    pipeline
        .execute(&mut ctx, final_handler)
        .await
        .expect("the request succeeds");

    // The root span's trace id must be the same value the middleware wrote to
    // the response header and stored on the context for handlers.
    let context_trace_id = ctx
        .trace_id()
        .expect("RequestTracing stores a trace id")
        .to_string();
    let header_trace_id = ctx
        .response()
        .header("x-trace-id")
        .expect("RequestTracing echoes x-trace-id")
        .to_string();
    assert_eq!(context_trace_id, REQUEST_TRACE_ID);
    assert_eq!(header_trace_id, context_trace_id);
    assert_eq!(ctx.response().status(), 200);

    let root = capture
        .request_span(ORDERS_PATH)
        .expect("root span captured");
    assert_eq!(root.field("method"), Some("GET"));
    assert_eq!(root.field("path"), Some(ORDERS_PATH));
    assert_eq!(root.field("trace_id"), Some(REQUEST_TRACE_ID));
    assert_eq!(root.field("status"), Some("200"));

    // Layer 1: authentication.
    let auth = capture
        .child_span("auth.authenticate", "http.server.request")
        .expect("auth span is a child of the root span");
    assert_eq!(auth.field("scheme"), Some("test-scheme"));
    assert_eq!(auth.field("outcome"), Some("ok"));

    // Layer 2: routing.
    let route = capture
        .child_span("http.route_match", "http.server.request")
        .expect("route span is a child of the root span");
    assert_eq!(route.field("pattern"), Some(ORDERS_PATH));
    assert_eq!(route.field("params"), Some("0"));

    // Layer 3: endpoint dispatch.
    let handler = capture
        .child_span("http.handler", "http.route_match")
        .expect("handler span is a child of the route span");
    assert_eq!(
        handler.field("handler"),
        Some("observability_test::OrdersRequest")
    );
    assert_eq!(handler.field("status"), Some("200"));

    // Layer 4: request-body binding under the handler.
    let binding = capture
        .child_span("http.binding", "http.handler")
        .expect("binding span is a child of the handler span");
    assert_eq!(binding.field("kind"), Some("bytes"));
    assert_eq!(binding.field("bytes"), Some("0"));
}

// ---------------------------------------------------------------------------
// Scenario 2: SSE connection lifecycle events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_long_connection_emits_opened_and_closed_events() {
    let capture = capture();

    let (parts, _) = http::Request::builder()
        .method("GET")
        .uri(SSE_PATH)
        .body(())
        .expect("test request builds")
        .into_parts();
    let mut ctx = EngineContext::from_parts(
        parts,
        Bytes::new(),
        Arc::new(BodyLimits::default()),
        None,
        None,
    );
    ctx.set_trace_id(SSE_TRACE_ID);

    let events = futures_util::stream::iter(vec![
        Ok::<SseEvent, webx_core::error::Error>(SseEvent::data("one")),
        Ok(SseEvent::data("two")),
    ]);
    ctx.write_endpoint_result(EndpointResult::Action(Box::new(SseResult::new(events))))
        .await
        .expect("the SSE result writes the response");
    let response = ctx.finalize().await;

    // `finalize` is where the long connection opens.
    let opened = wait_for("connection.opened", || {
        capture
            .events_named("connection.opened")
            .into_iter()
            .find(|event| {
                event.field("transport") == Some("sse")
                    && event.field("trace_id") == Some(SSE_TRACE_ID)
            })
    })
    .await;
    assert_eq!(opened.field("path"), Some(SSE_PATH));
    let connection_id = opened
        .field("connection_id")
        .expect("the event carries the connection id")
        .to_string();
    assert!(!connection_id.is_empty());

    // Draining the body is the client consuming the event stream; the wrapper
    // sees the end of the stream and closes the connection.
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("the SSE body drains")
        .to_bytes();
    assert!(
        body.windows(3).any(|window| window == b"one"),
        "the SSE payload reached the body"
    );

    let closed = wait_for("connection.closed", || {
        capture
            .events_named("connection.closed")
            .into_iter()
            .find(|event| event.field("connection_id") == Some(connection_id.as_str()))
    })
    .await;
    assert_eq!(closed.field("transport"), Some("sse"));
    assert_eq!(closed.field("trace_id"), Some(SSE_TRACE_ID));
    assert_eq!(closed.field("reason"), Some("completed"));
    assert!(
        closed.field("duration_ms").is_some(),
        "the closed event carries the connection duration"
    );
    assert_eq!(
        capture
            .events_named("connection.closed")
            .into_iter()
            .filter(|event| event.field("connection_id") == Some(connection_id.as_str()))
            .count(),
        1,
        "connection.closed is emitted exactly once"
    );
}

// ---------------------------------------------------------------------------
// Scenario 3: WebSocket connection lifecycle events
// ---------------------------------------------------------------------------

/// Receive until the peer closes; the framework ends the session afterwards.
struct QuietHandler;

#[async_trait::async_trait]
impl IWebSocketHandler for QuietHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> WebxResult<()> {
        while let Some(message) = connection.recv().await {
            if matches!(message?, RealtimeMessage::Close { .. }) {
                break;
            }
        }
        Ok(())
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("local addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the host did not become ready");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_long_connection_emits_opened_and_closed_events() {
    let capture = capture();
    let port = free_port();
    let handler: Arc<dyn IWebSocketHandler> = Arc::new(QuietHandler);
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        // The trace id is assigned by the tracing middleware, then propagated to
        // the upgrade request's connection metadata (and thus the events).
        .use_middleware(MiddlewareSpec::new::<RequestTracing>())
        .use_websocket(WS_PATH, handler)
        .build();
    let server_handle = host.server_handle();
    let address = format!("127.0.0.1:{port}");
    let server = tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("client connect");
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://127.0.0.1:{port}{WS_PATH}")
        .into_client_request()
        .expect("upgrade request");
    request.headers_mut().insert(
        "x-request-id",
        WS_TRACE_ID.parse().expect("valid request id header"),
    );
    let (mut client, response) = tokio_tungstenite::client_async(request, stream)
        .await
        .expect("websocket handshake");
    assert_eq!(response.status(), 101);

    let opened = wait_for("websocket connection.opened", || {
        capture
            .events_named("connection.opened")
            .into_iter()
            .find(|event| {
                event.field("transport") == Some("websocket")
                    && event.field("trace_id") == Some(WS_TRACE_ID)
            })
    })
    .await;
    let connection_id = opened
        .field("connection_id")
        .expect("the event carries the connection id")
        .to_string();
    assert!(!connection_id.is_empty());

    // The upgrade request itself is a normal request with a root span; it is
    // the parent context the long connection's trace id came from.
    let upgrade_root = wait_for("the upgrade request root span", || {
        capture.request_span(WS_PATH)
    })
    .await;
    assert_eq!(upgrade_root.field("trace_id"), Some(WS_TRACE_ID));
    assert_eq!(upgrade_root.field("status"), Some("101"));

    // The peer closing ends the session; the framework emits connection.closed.
    client.close(None).await.expect("client close");
    let closed = wait_for("websocket connection.closed", || {
        capture
            .events_named("connection.closed")
            .into_iter()
            .find(|event| event.field("connection_id") == Some(connection_id.as_str()))
    })
    .await;
    assert_eq!(closed.field("transport"), Some("websocket"));
    assert_eq!(closed.field("trace_id"), Some(WS_TRACE_ID));
    assert_eq!(closed.field("reason"), Some("completed"));

    assert_eq!(
        capture
            .events_named("connection.closed")
            .into_iter()
            .filter(|event| event.field("connection_id") == Some(connection_id.as_str()))
            .count(),
        1,
        "connection.closed is emitted exactly once"
    );

    server_handle.shutdown();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the host stops within the test timeout")
        .expect("the host task does not panic");
}
