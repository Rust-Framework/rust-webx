//! Buffered responses: `OutboundResponse::from_bytes` / `ResponseStream::from_bytes`
//! must behave like a real upstream body on the read paths downstream uses
//! (`collect`, `text`, `json`).
//!
//! Offline: no sockets, no ports.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use webx_client::testing::ok_response;
use webx_client::{
    HttpClientError, HttpClientRegistry, IHttpClient, IHttpClientFactory, OutboundRequest,
    OutboundResponse, ResponseStream,
};

fn json_headers() -> Vec<(String, String)> {
    vec![("content-type".into(), "application/json".into())]
}

#[tokio::test]
async fn buffered_response_reads_body() {
    let response =
        OutboundResponse::from_bytes(200, json_headers(), Bytes::from_static(b"{\"ok\":true}"))
            .expect("valid response");
    assert!(response.is_success());
    assert_eq!(response.status, 200);
    assert_eq!(response.version, http::Version::HTTP_11);
    assert_eq!(response.headers["content-type"], "application/json");

    let value: serde_json::Value = response.body.json(1024).await.expect("json body");
    assert_eq!(value, serde_json::json!({ "ok": true }));

    let response =
        OutboundResponse::from_bytes(200, json_headers(), Bytes::from_static(b"{\"ok\":true}"))
            .expect("valid response");
    assert_eq!(
        response.body.text(1024).await.expect("text body"),
        "{\"ok\":true}"
    );
}

#[tokio::test]
async fn buffered_collect_enforces_limit() {
    let response =
        OutboundResponse::from_bytes(200, Vec::new(), vec![b'x'; 64]).expect("valid response");
    let error = response
        .body
        .collect(16)
        .await
        .expect_err("body exceeds the limit");
    assert!(matches!(error, HttpClientError::BodyTooLarge { limit: 16 }));
}

#[tokio::test]
async fn buffered_empty_body() {
    let response = OutboundResponse::from_bytes(204, Vec::new(), Bytes::new()).expect("valid");
    assert_eq!(response.body.text(1024).await.expect("text body"), "");

    let response = OutboundResponse::from_bytes(204, Vec::new(), Vec::<u8>::new()).expect("valid");
    let body = response.body.collect(1024).await.expect("body collects");
    assert!(body.is_empty());

    // The stream constructor used directly by a fake response struct.
    let body = ResponseStream::from_bytes(Bytes::new())
        .collect(0)
        .await
        .expect("an empty body fits any limit");
    assert!(body.is_empty());
}

#[test]
fn invalid_header_is_rejected() {
    for headers in [
        // Space in the header name.
        vec![("bad header".to_string(), "value".to_string())],
        // Control characters in the value.
        vec![("x-test".to_string(), "bad\nvalue".to_string())],
        vec![("x-test".to_string(), "\u{0}".to_string())],
    ] {
        let error = OutboundResponse::from_bytes(200, headers, Bytes::new())
            .expect_err("illegal header must be rejected");
        assert!(
            matches!(error, HttpClientError::InvalidRequest(_)),
            "unexpected error: {error:?}"
        );
    }
}

struct FakeClient {
    responses: Mutex<VecDeque<OutboundResponse>>,
}

impl FakeClient {
    fn new(responses: Vec<OutboundResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl IHttpClient for FakeClient {
    fn name(&self) -> &str {
        "fake"
    }

    async fn execute(
        &self,
        _request: OutboundRequest,
    ) -> Result<OutboundResponse, HttpClientError> {
        let response = self
            .responses
            .lock()
            .expect("fake response queue is not poisoned")
            .pop_front()
            .expect("a scripted response is queued");
        Ok(response)
    }
}

#[tokio::test]
async fn fake_http_client_returns_buffered_success() {
    let fake: Arc<dyn IHttpClient> = Arc::new(FakeClient::new(vec![OutboundResponse::from_bytes(
        200,
        json_headers(),
        Bytes::from_static(br#"{"access_token":"abc","token_type":"Bearer"}"#),
    )
    .expect("valid response")]));
    let registry = HttpClientRegistry::new().with("idp", Arc::clone(&fake));
    let factory: &dyn IHttpClientFactory = &registry;

    let client = factory.create("idp").expect("registered client resolves");
    let response = client
        .execute(OutboundRequest::post("/token"))
        .await
        .expect("fake succeeds without a socket");
    assert!(response.is_success());
    let value: serde_json::Value = response.body.json(4096).await.expect("json body");
    assert_eq!(value["access_token"], "abc");
    assert_eq!(value["token_type"], "Bearer");
}

#[tokio::test]
async fn testing_helper_builds_an_ok_response() {
    let response = ok_response(Bytes::from_static(br#"{"ok":true}"#));
    assert!(response.is_success());
    assert_eq!(response.headers["content-type"], "application/json");
    let value: serde_json::Value = response.body.json(1024).await.expect("json body");
    assert_eq!(value, serde_json::json!({ "ok": true }));
}
