//! `OutboundRequest` read accessors.
//!
//! A fake `IHttpClient` has to inspect the request it was handed (method, path,
//! headers, buffered body) while still returning a real
//! `webx_client::OutboundResponse` via `OutboundResponse::from_bytes`. These
//! accessors exist for exactly that: fake clients, middleware and diagnostics.

use std::time::Duration;

use webx_client::{HttpClientError, OutboundRequest};

#[test]
fn buffered_json_body_is_inspectable() {
    let request = OutboundRequest::post("https://billing.example.com/oauth2/token")
        .header("accept", "application/json")
        .json(&serde_json::json!({ "grant_type": "client_credentials" }))
        .expect("serialize body")
        .timeout(Duration::from_secs(3));

    assert_eq!(request.method(), &http::Method::POST);
    assert_eq!(request.path(), "https://billing.example.com/oauth2/token");
    assert_eq!(
        request.headers(),
        &[
            ("accept".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
    );
    assert_eq!(request.request_timeout(), Some(Duration::from_secs(3)));

    let body = request.body_bytes().expect("buffered body");
    let parsed: serde_json::Value = serde_json::from_slice(body).expect("JSON body");
    assert_eq!(parsed["grant_type"], "client_credentials");
}

#[test]
fn buffered_bytes_body_is_inspectable() {
    let request =
        OutboundRequest::put("https://api.example.com/items").body(b"{\"n\":1}".as_slice());
    assert_eq!(request.body_bytes(), Some(&b"{\"n\":1}"[..]));
    assert!(request.request_timeout().is_none());
}

#[test]
fn streamed_body_is_not_reported_as_buffered() {
    let chunks: Vec<Result<bytes::Bytes, HttpClientError>> =
        vec![Ok(bytes::Bytes::from_static(b"chunk"))];
    let request = OutboundRequest::post("https://api.example.com/upload")
        .body_stream(futures_util::stream::iter(chunks));

    assert!(
        request.body_bytes().is_none(),
        "a streamed body cannot be inspected without consuming it"
    );
    assert_eq!(request.method(), &http::Method::POST);
}
