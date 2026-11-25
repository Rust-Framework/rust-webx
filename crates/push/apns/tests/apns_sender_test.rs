//! APNs adapter tests against the scripted loopback mock upstream.
//!
//! The mock binds an ephemeral `127.0.0.1` port and serves queued responses, so
//! nothing here touches the network or a real APNs host. `IHttpClient` is
//! satisfied by the framework's real client pointed at that origin, which means
//! the assertions cover the actual request shape (method, path, headers, body).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::json;
use webx_apns::{ApnsConfig, ApnsEnvironment, ApnsPushSender};
use webx_client::testing::{
    MockUpstream, RecordedRequest, RecordedResponse, ReplayExchange, ReplayFixture, ReplayRequest,
    FIXTURE_VERSION,
};
use webx_client::{HttpClient, HttpClientOptions};
use webx_core::push::{IPushSender, PushMessage, PushOutcome, PushPlatform};

const TEAM_ID: &str = "TEAM123456";
const KEY_ID: &str = "KEY1234567";
const TOPIC: &str = "com.example.app";
const DEVICE_TOKEN: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

/// Test-only ES256 key. Never used for a real Apple account.
const EC_PRIVATE_PEM: &str = r"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgveGKu9Urfqr7jzNG
8a17TMH3zaNDA2op2dFMe578DK6hRANCAATN38lNJ4nAVvBg/bHU1ohg737ABylJ
xnA/+REqxj5yZEVJGb52OFYyk+cH69d4EVPvE8U43X+pZCTP5Ju5rDCn
-----END PRIVATE KEY-----
";

/// Public half of [`EC_PRIVATE_PEM`], used only to verify minted tokens.
const EC_PUBLIC_PEM: &str = r"-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEzd/JTSeJwFbwYP2x1NaIYO9+wAcp
ScZwP/kRKsY+cmRFSRm+djhWMpPnB+vXeBFT7xPFON1/qWQkz+Sbuawwpw==
-----END PUBLIC KEY-----
";

fn config() -> ApnsConfig {
    ApnsConfig::new(
        TEAM_ID,
        KEY_ID,
        EC_PRIVATE_PEM,
        TOPIC,
        ApnsEnvironment::Sandbox,
    )
}

fn sender(mock: &MockUpstream) -> ApnsPushSender {
    let client = HttpClient::new("apns-test", HttpClientOptions::new(mock.base_url()))
        .expect("mock client builds");
    ApnsPushSender::new(config(), Arc::new(client)).expect("sender builds")
}

/// Queue one response with an arbitrary status, headers and body.
fn script(mock: &MockUpstream, status: u16, headers: &[(&str, &str)], body: &str) {
    let fixture = ReplayFixture {
        version: FIXTURE_VERSION,
        exchanges: vec![ReplayExchange {
            request: ReplayRequest {
                method: String::new(),
                path: String::new(),
                headers: Vec::new(),
                body: String::new(),
            },
            response: RecordedResponse {
                status,
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
                body: body.to_owned(),
            },
        }],
    };
    mock.replay(&fixture);
}

fn header<'a>(request: &'a RecordedRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn bearer_token(request: &RecordedRequest) -> &str {
    header(request, "authorization")
        .expect("authorization header")
        .strip_prefix("Bearer ")
        .expect("bearer authentication")
}

fn verified_claims(token: &str) -> serde_json::Value {
    let mut validation = Validation::new(Algorithm::ES256);
    // APNs provider tokens carry `iss`/`iat` only (no `exp`), so `exp` must not
    // be required; the signature and issuer are still verified.
    validation.required_spec_claims = HashSet::new();
    validation.set_issuer(&[TEAM_ID]);
    let key = DecodingKey::from_ec_pem(EC_PUBLIC_PEM.as_bytes()).expect("test public key");
    jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation)
        .expect("provider token verifies")
        .claims
}

#[tokio::test]
async fn an_alert_push_sets_the_apns_request_shape() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(200, &json!({}));
    let message = PushMessage::alert(PushPlatform::Apns, DEVICE_TOKEN, "Hello", "World")
        .with_data(json!({ "thread-id": "chat-1", "aps": { "badge": 99 } }))
        .with_collapse_key("chat-1")
        .with_ttl(Duration::from_secs(60));

    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let outcome = sender(&mock).send(message).await.expect("push accepted");
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    assert_eq!(outcome, PushOutcome::Accepted);

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, format!("/3/device/{DEVICE_TOKEN}"));
    assert_eq!(header(request, "apns-topic"), Some(TOPIC));
    assert_eq!(header(request, "apns-push-type"), Some("alert"));
    assert_eq!(header(request, "apns-priority"), Some("10"));
    assert_eq!(header(request, "apns-collapse-id"), Some("chat-1"));

    let expiration: u64 = header(request, "apns-expiration")
        .expect("apns-expiration header")
        .parse()
        .expect("unix seconds");
    // The header is `now + ttl`, minted somewhere between `before` and `after`.
    assert!((before + 60..=after + 60).contains(&expiration));

    let token = bearer_token(request);
    let jwt_header = jsonwebtoken::decode_header(token).expect("jwt header");
    assert_eq!(jwt_header.alg, Algorithm::ES256);
    assert_eq!(jwt_header.kid.as_deref(), Some(KEY_ID));
    let claims = verified_claims(token);
    assert_eq!(claims["iss"], TEAM_ID);
    assert!(claims["iat"].as_u64().expect("iat") > 0);

    let payload: serde_json::Value =
        serde_json::from_slice(&request.body).expect("request payload is JSON");
    assert_eq!(payload["aps"]["alert"]["title"], "Hello");
    assert_eq!(payload["aps"]["alert"]["body"], "World");
    assert_eq!(payload["thread-id"], "chat-1");
    // Application data cannot override the protocol-reserved `aps` dictionary.
    assert!(payload["aps"]["badge"].is_null());
}

#[tokio::test]
async fn a_data_only_push_is_a_silent_background_request() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(200);
    let message =
        PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN).with_data(json!({ "sync": "inbox" }));

    let outcome = sender(&mock).send(message).await.expect("push accepted");
    assert_eq!(outcome, PushOutcome::Accepted);

    let requests = mock.requests();
    let request = &requests[0];
    assert_eq!(header(request, "apns-push-type"), Some("background"));
    assert_eq!(header(request, "apns-priority"), Some("5"));
    assert!(header(request, "apns-expiration").is_none());
    assert!(header(request, "apns-collapse-id").is_none());
    let payload: serde_json::Value =
        serde_json::from_slice(&request.body).expect("request payload is JSON");
    assert_eq!(payload["aps"]["content-available"], 1);
    assert_eq!(payload["sync"], "inbox");
}

#[tokio::test]
async fn a_two_hundred_one_is_accepted() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(201);
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("push accepted"),
        PushOutcome::Accepted
    );
}

#[tokio::test]
async fn the_provider_token_is_cached_between_sends() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(200, &json!({}));
    mock.json(200, &json!({}));
    let sender = sender(&mock);

    sender
        .send(PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN))
        .await
        .expect("first send");
    sender
        .send(PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN))
        .await
        .expect("second send");

    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    // ECDSA signatures are randomized, so an identical bearer token proves the
    // cached provider token was reused instead of minting a second one.
    assert_eq!(bearer_token(&requests[0]), bearer_token(&requests[1]));
}

#[tokio::test]
async fn concurrent_sends_share_one_provider_token() {
    let mock = MockUpstream::start().await.expect("mock starts");
    for _ in 0..4 {
        mock.json(200, &json!({}));
    }
    let sender = Arc::new(sender(&mock));

    let mut tasks = Vec::new();
    for _ in 0..4 {
        let sender = Arc::clone(&sender);
        tasks.push(tokio::spawn(async move {
            sender
                .send(PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN))
                .await
        }));
    }
    for task in tasks {
        assert_eq!(
            task.await.expect("task joins").expect("send"),
            PushOutcome::Accepted
        );
    }

    let requests = mock.requests();
    assert_eq!(requests.len(), 4);
    let tokens: HashSet<&str> = requests.iter().map(bearer_token).collect();
    assert_eq!(
        tokens.len(),
        1,
        "concurrent sends must reuse one provider token"
    );
}

#[tokio::test]
async fn a_bad_device_token_is_token_invalid() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(400, &json!({ "reason": "BadDeviceToken" }));
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::TokenInvalid
    );
}

#[tokio::test]
async fn a_token_for_another_topic_is_token_invalid() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(400, &json!({ "reason": "DeviceTokenNotForTopic" }));
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::TokenInvalid
    );
}

#[tokio::test]
async fn an_unregistered_token_is_token_invalid() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(410);
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::TokenInvalid
    );
}

#[tokio::test]
async fn rate_limiting_is_retryable_with_the_retry_after() {
    let mock = MockUpstream::start().await.expect("mock starts");
    script(&mock, 429, &[("retry-after", "7")], "{}");
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::Retryable {
            retry_after: Some(Duration::from_secs(7)),
        }
    );
}

#[tokio::test]
async fn an_unavailable_service_is_retryable() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(503);
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::Retryable { retry_after: None }
    );
}

#[tokio::test]
async fn an_expired_provider_token_clears_the_cache() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(400, &json!({ "reason": "ExpiredProviderToken" }));
    mock.json(200, &json!({}));
    let sender = sender(&mock);

    let failed = sender
        .send(PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN))
        .await
        .expect("expired token is a verdict");
    assert_eq!(failed, PushOutcome::Retryable { retry_after: None });

    let accepted = sender
        .send(PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN))
        .await
        .expect("second attempt");
    assert_eq!(accepted, PushOutcome::Accepted);

    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    // A fresh provider token was minted for the retry.
    assert_ne!(bearer_token(&requests[0]), bearer_token(&requests[1]));
    assert_eq!(verified_claims(bearer_token(&requests[1]))["iss"], TEAM_ID);
}

#[tokio::test]
async fn rejected_credentials_are_a_configuration_error_without_the_device_token() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(401);
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("credentials are a configuration failure");

    assert_eq!(error.status_code(), 503);
    assert!(!error.to_string().contains(DEVICE_TOKEN));
    assert!(error
        .public_detail()
        .is_none_or(|detail| !detail.contains(DEVICE_TOKEN)));
}

#[tokio::test]
async fn a_bare_forbidden_is_a_credentials_error() {
    let mock = MockUpstream::start().await.expect("mock starts");
    // APNs rejects the provider token itself with 403 and may omit the body.
    mock.status(403);
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("forbidden credentials");
    assert_eq!(error.status_code(), 503);
    assert!(error.retryable());
}

#[tokio::test]
async fn a_platform_mismatch_is_a_validation_error_and_sends_nothing() {
    let mock = MockUpstream::start().await.expect("mock starts");
    // Valid FCM message: validation succeeds, the platform check must reject it.
    let message = PushMessage::silent(PushPlatform::Fcm, "device-token");
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("platform mismatch");
    assert_eq!(error.code(), "validation_failed");
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn an_invalid_message_is_rejected_before_any_http_request() {
    let mock = MockUpstream::start().await.expect("mock starts");
    let error = sender(&mock)
        .send(PushMessage::silent(PushPlatform::Apns, ""))
        .await
        .expect_err("empty token is invalid");
    assert_eq!(error.code(), "validation_failed");
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn a_malformed_signing_key_fails_at_construction() {
    let mock = MockUpstream::start().await.expect("mock starts");
    let client = HttpClient::new("apns-test", HttpClientOptions::new(mock.base_url()))
        .expect("mock client builds");
    let config = ApnsConfig::new(
        TEAM_ID,
        KEY_ID,
        "-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----\n",
        TOPIC,
        ApnsEnvironment::Sandbox,
    );
    let error = ApnsPushSender::new(config, Arc::new(client))
        .err()
        .expect("invalid PEM fails at construction");
    assert_eq!(error.status_code(), 500);
    assert!(error.public_detail().is_none());
    assert!(!error.to_string().contains("not-a-key"));
    assert!(mock.requests().is_empty());
}
