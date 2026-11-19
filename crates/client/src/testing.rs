//! Scripted upstream for integration tests and record/replay.
//!
//! [`MockUpstream`] binds an ephemeral HTTP/1.1 port and serves queued
//! responses. It is intentionally small: JSON, raw status, and SSE chunks, plus
//! a record of every request (method, path, headers, body). It is not a
//! production server.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::{Request, Response};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

type MockBody = UnsyncBoxBody<Bytes, Infallible>;

/// One request recorded by the mock.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// Method (GET/POST/...).
    pub method: String,
    /// Path plus query string.
    pub path: String,
    /// Header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// Raw request body.
    pub body: Bytes,
}

/// One response recorded by the mock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedResponse {
    /// HTTP status.
    pub status: u16,
    /// Response headers (excluding the mock's synthetic delay header).
    pub headers: Vec<(String, String)>,
    /// Response body as UTF-8 (lossy for binary fixtures).
    pub body: String,
}

/// One replayable request/response pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayExchange {
    /// Request that produced the response.
    pub request: ReplayRequest,
    /// Recorded response.
    pub response: RecordedResponse,
}

/// Serializable request side of a fixture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRequest {
    /// Method.
    pub method: String,
    /// Path plus query.
    pub path: String,
    /// Headers.
    pub headers: Vec<(String, String)>,
    /// Body as UTF-8 (lossy for binary fixtures).
    pub body: String,
}

/// Current record/replay fixture format version.
///
/// Version 1 is the implicit pre-versioning format (files without a `version`
/// field). Version 2 adds the version field and redaction-aware saving; loading
/// accepts 1..=2 and rejects newer files instead of misreading them.
pub const FIXTURE_VERSION: u32 = 2;

/// Headers whose values are replaced by `<redacted>` when a fixture is saved.
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "cookie",
    "set-cookie",
];

/// Serializable record/replay fixture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayFixture {
    /// Fixture format version ([`FIXTURE_VERSION`] for new recordings).
    #[serde(default = "legacy_fixture_version")]
    pub version: u32,
    /// Captured exchanges, in order.
    pub exchanges: Vec<ReplayExchange>,
}

fn legacy_fixture_version() -> u32 {
    1
}

impl Default for ReplayFixture {
    fn default() -> Self {
        Self {
            version: FIXTURE_VERSION,
            exchanges: Vec::new(),
        }
    }
}

impl ReplayFixture {
    /// A copy with credential-bearing headers replaced by `<redacted>`.
    ///
    /// Bodies and non-credential headers are kept verbatim; redact prompt data
    /// in the fixture itself if the recording contains it.
    pub fn redacted(&self) -> Self {
        let mut copy = self.clone();
        for exchange in &mut copy.exchanges {
            redact_headers(&mut exchange.request.headers);
            redact_headers(&mut exchange.response.headers);
        }
        copy
    }

    /// Reject fixtures produced by a newer library version.
    fn check_version(&self) -> std::io::Result<()> {
        if self.version > FIXTURE_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "fixture version {} is newer than supported version {}",
                    self.version, FIXTURE_VERSION
                ),
            ));
        }
        Ok(())
    }
}

fn redact_headers(headers: &mut [(String, String)]) {
    for (name, value) in headers {
        if SENSITIVE_HEADERS
            .iter()
            .any(|sensitive| name.eq_ignore_ascii_case(sensitive))
        {
            *value = "<redacted>".to_string();
        }
    }
}

struct Scripted {
    status: u16,
    headers: Vec<(String, String)>,
    body: MockBody,
    record_body: Vec<u8>,
}

struct State {
    scripts: Mutex<VecDeque<Scripted>>,
    requests: Mutex<Vec<RecordedRequest>>,
    responses: Mutex<Vec<RecordedResponse>>,
}

/// Scripted upstream bound to an ephemeral local port.
pub struct MockUpstream {
    base_url: String,
    state: Arc<State>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl MockUpstream {
    /// Start the mock and return its base URL (`http://127.0.0.1:<port>`).
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let state = Arc::new(State {
            scripts: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
        });
        let (shutdown, mut rx) = watch::channel(false);
        let accept_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = rx.changed() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let state = Arc::clone(&accept_state);
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            let _ = http1::Builder::new()
                                .serve_connection(io, service_fn(move |req| {
                                    handle(Arc::clone(&state), req)
                                }))
                                .await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            base_url,
            state,
            shutdown,
            task,
        })
    }

    /// Base URL to pass to [`HttpClientOptions::new`](crate::HttpClientOptions::new).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Queue a JSON response.
    pub fn json<T: Serialize>(&self, status: u16, value: &T) {
        let bytes = serde_json::to_vec(value).unwrap_or_default();
        self.push_script(
            status,
            vec![("content-type".into(), "application/json".into())],
            body_from_bytes(bytes.clone()),
            bytes,
        );
    }

    /// Queue a full SSE response from raw `data:` chunks.
    pub fn sse<I, S>(&self, chunks: I)
    where
        I: IntoIterator<Item = S>,
        I::IntoIter: Send + 'static,
        S: Into<String>,
    {
        let mut normalized = Vec::new();
        let mut recorded = Vec::new();
        for chunk in chunks {
            let mut bytes = chunk.into().into_bytes();
            if !bytes.ends_with(b"\n\n") {
                bytes.extend_from_slice(b"\n\n");
            }
            recorded.extend_from_slice(&bytes);
            normalized.push(bytes);
        }
        let frames = normalized
            .into_iter()
            .map(|bytes| Ok::<Frame<Bytes>, Infallible>(Frame::data(Bytes::from(bytes))));
        let body = StreamBody::new(stream::iter(frames)).boxed_unsync();
        self.push_script(
            200,
            vec![(
                "content-type".into(),
                "text/event-stream; charset=utf-8".into(),
            )],
            body,
            recorded,
        );
    }

    /// Queue an empty response with `status`.
    pub fn status(&self, status: u16) {
        self.push_script(status, Vec::new(), body_from_bytes(Vec::new()), Vec::new());
    }

    /// Delay applied before the next queued response is sent (one-shot).
    pub fn delay_before_next(&self, delay: Duration) {
        if let Some(script) = self
            .state
            .scripts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .front_mut()
        {
            script
                .headers
                .push(("x-mock-delay-ms".into(), delay.as_millis().to_string()));
        }
    }

    /// Every request recorded so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Recorded request/response pairs, ready for serialization.
    pub fn fixture(&self) -> ReplayFixture {
        let requests = self.requests();
        let responses = self
            .state
            .responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        ReplayFixture {
            version: FIXTURE_VERSION,
            exchanges: requests
                .into_iter()
                .zip(responses)
                .map(|(request, response)| ReplayExchange {
                    request: ReplayRequest {
                        method: request.method,
                        path: request.path,
                        headers: request.headers,
                        body: String::from_utf8_lossy(&request.body).to_string(),
                    },
                    response,
                })
                .collect(),
        }
    }

    /// Write the fixture as pretty JSON with credential headers redacted.
    ///
    /// This is the default save path so a recorded `authorization` header does
    /// not end up in git. Use [`MockUpstream::save_fixture_raw`] only for a
    /// deliberately local, never-committed capture.
    pub fn save_fixture(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let bytes =
            serde_json::to_vec_pretty(&self.fixture().redacted()).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    }

    /// Write the fixture verbatim (secrets included).
    pub fn save_fixture_raw(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.fixture()).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    }

    /// Load a fixture from JSON; a newer format version is rejected.
    pub fn load_fixture(path: impl AsRef<std::path::Path>) -> std::io::Result<ReplayFixture> {
        let bytes = std::fs::read(path)?;
        let fixture: ReplayFixture =
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        fixture.check_version()?;
        Ok(fixture)
    }

    /// Queue every response in `fixture`.
    pub fn replay(&self, fixture: &ReplayFixture) {
        for exchange in &fixture.exchanges {
            let bytes = exchange.response.body.clone().into_bytes();
            self.push_script(
                exchange.response.status,
                exchange.response.headers.clone(),
                body_from_bytes(bytes.clone()),
                bytes,
            );
        }
    }

    /// Stop accepting and abort in-flight mock connections.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }

    fn push_script(
        &self,
        status: u16,
        headers: Vec<(String, String)>,
        body: MockBody,
        record_body: Vec<u8>,
    ) {
        self.state
            .scripts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(Scripted {
                status,
                headers,
                body,
                record_body,
            });
    }
}

impl Drop for MockUpstream {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}

async fn handle(
    state: Arc<State>,
    req: Request<hyper::body::Incoming>,
) -> Result<Response<MockBody>, Infallible> {
    let method = req.method().to_string();
    let path = req.uri().to_string();
    let headers = req
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<Vec<_>>();
    let body = req
        .into_body()
        .collect()
        .await
        .map(|body| body.to_bytes())
        .unwrap_or_default();
    state
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(RecordedRequest {
            method,
            path,
            headers,
            body,
        });

    let script = state
        .scripts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .pop_front();

    let Some(script) = script else {
        state
            .responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedResponse {
                status: 503,
                headers: Vec::new(),
                body: "no scripted response".to_string(),
            });
        return Ok(Response::builder()
            .status(503)
            .body(body_from_bytes(b"no scripted response".to_vec()))
            .expect("response builds"));
    };
    let recorded_headers = script
        .headers
        .iter()
        .filter(|(name, _)| name != "x-mock-delay-ms")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    state
        .responses
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(RecordedResponse {
            status: script.status,
            headers: recorded_headers,
            body: String::from_utf8_lossy(&script.record_body).to_string(),
        });
    // A tiny delay hook for timeout tests; encoded in a synthetic header so the
    // public API stays a plain queue.
    if let Some(delay) = script
        .headers
        .iter()
        .find(|(name, _)| name == "x-mock-delay-ms")
        .and_then(|(_, value)| value.parse::<u64>().ok())
    {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    let mut builder = Response::builder().status(script.status);
    for (name, value) in script.headers {
        if name != "x-mock-delay-ms" {
            builder = builder.header(name, value);
        }
    }
    Ok(builder.body(script.body).expect("response builds"))
}

fn body_from_bytes(bytes: Vec<u8>) -> MockBody {
    Full::new(Bytes::from(bytes))
        .map_err(|never| match never {})
        .boxed_unsync()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};

    #[tokio::test]
    async fn captures_and_replays_a_fixture() {
        let mock = MockUpstream::start().await.expect("mock starts");
        mock.json(200, &serde_json::json!({ "ok": true }));
        let client = HttpClient::new("mock", HttpClientOptions::new(mock.base_url()))
            .expect("client builds");
        let response = client
            .execute(OutboundRequest::get("/json"))
            .await
            .expect("request succeeds");
        assert_eq!(
            &response.body.collect(1024).await.expect("body")[..],
            b"{\"ok\":true}"
        );

        let fixture = mock.fixture();
        assert_eq!(fixture.version, FIXTURE_VERSION);
        assert_eq!(fixture.exchanges.len(), 1);
        assert_eq!(fixture.exchanges[0].request.path, "/json");
        assert_eq!(fixture.exchanges[0].response.status, 200);

        let path = std::env::temp_dir().join(format!(
            "webx-mock-fixture-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.subsec_nanos())
                .unwrap_or_default()
        ));
        mock.save_fixture(&path).expect("save fixture");
        let loaded = MockUpstream::load_fixture(&path).expect("load fixture");
        let _ = std::fs::remove_file(path);

        let replay = MockUpstream::start().await.expect("replay starts");
        replay.replay(&loaded);
        let client = HttpClient::new("replay", HttpClientOptions::new(replay.base_url()))
            .expect("client builds");
        let response = client
            .execute(OutboundRequest::get("/json"))
            .await
            .expect("request succeeds");
        assert_eq!(
            &response.body.collect(1024).await.expect("body")[..],
            b"{\"ok\":true}"
        );
    }

    fn temp_fixture_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "webx-mock-{tag}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ))
    }

    #[tokio::test]
    async fn saving_redacts_credentials_by_default() {
        let mock = MockUpstream::start().await.expect("mock starts");
        mock.json(200, &serde_json::json!({ "ok": true }));
        let client = HttpClient::new("mock", HttpClientOptions::new(mock.base_url()))
            .expect("client builds");
        client
            .execute(
                OutboundRequest::get("/json")
                    .header("authorization", "Bearer sk-secret")
                    .header("x-api-key", "sk-secret")
                    .header("x-request-id", "req-1"),
            )
            .await
            .expect("request succeeds");

        let fixture = mock.fixture();
        let redacted = fixture.redacted();
        let auth = &redacted.exchanges[0].request.headers;
        assert!(auth
            .iter()
            .any(|(name, value)| name == "authorization" && value == "<redacted>"));
        assert!(auth
            .iter()
            .any(|(name, value)| name == "x-api-key" && value == "<redacted>"));
        assert!(auth
            .iter()
            .any(|(name, value)| name == "x-request-id" && value == "req-1"));

        let safe = temp_fixture_path("safe");
        mock.save_fixture(&safe).expect("save redacted");
        let text = std::fs::read_to_string(&safe).expect("read redacted");
        assert!(!text.contains("sk-secret"), "credentials leaked: {text}");
        assert!(text.contains("<redacted>"));
        let _ = std::fs::remove_file(safe);

        let raw = temp_fixture_path("raw");
        mock.save_fixture_raw(&raw).expect("save raw");
        let text = std::fs::read_to_string(&raw).expect("read raw");
        assert!(text.contains("sk-secret"));
        let _ = std::fs::remove_file(raw);
    }

    #[test]
    fn loading_a_newer_fixture_version_is_an_error() {
        let path = temp_fixture_path("future");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": FIXTURE_VERSION + 1,
                "exchanges": []
            }))
            .expect("json"),
        )
        .expect("write");
        let error =
            MockUpstream::load_fixture(&path).expect_err("a newer fixture must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_fixture_without_a_version_loads_as_version_one() {
        let path = temp_fixture_path("legacy");
        std::fs::write(&path, br#"{"exchanges":[]}"#).expect("write");
        let loaded = MockUpstream::load_fixture(&path).expect("legacy loads");
        assert_eq!(loaded.version, 1);
        assert!(loaded.exchanges.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
