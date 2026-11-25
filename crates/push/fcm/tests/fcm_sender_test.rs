//! FCM adapter tests against the scripted loopback mock upstream.
//!
//! The mock binds an ephemeral `127.0.0.1` port and serves queued responses, so
//! nothing here touches the network or Google. The service-account `token_uri`
//! points at the mock, and `messages:send` is posted relative to the injected
//! client's base URL, so both legs exercise the real request code.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::json;
use webx_client::testing::{
    MockUpstream, RecordedRequest, RecordedResponse, ReplayExchange, ReplayFixture, ReplayRequest,
    FIXTURE_VERSION,
};
use webx_client::{HttpClient, HttpClientOptions};
use webx_core::push::{IPushSender, PushMessage, PushOutcome, PushPlatform};
use webx_fcm::{FcmPushSender, FcmServiceAccount};

const PROJECT_ID: &str = "demo-project";
const CLIENT_EMAIL: &str = "svc@demo-project.iam.gserviceaccount.com";
const DEVICE_TOKEN: &str = "fcm-device-token";
const MESSAGES_PATH: &str = "/v1/projects/demo-project/messages:send";

/// Test-only RS256 key. Never used for a real Firebase project.
const RSA_PRIVATE_PEM: &str = r"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC0rhcWQxL9i3+n
k5SqbWTe7xTD/xRI4mVI9Lo1srmbt5ret0y3tv0+yVnxGa/jPZ5VeH9Q2a11CjMY
84JUsYDZfIbC84szaVJkKoBS6Ze4FF6mg4/+vICW8zxloDVvLQZ+LRYEw5Wr5+aR
oS9s3wtw4oRWbtxlCiohtvwOFLhX5ku8YlWbxfp9t560yUiJ0pBFdQnFKqkOqYJ8
AzVZw+3YUTadGY3zDMoJiljYw+RAXVQa83ROMbdiZMSVFm+8T2G3leUmIPi2b1sV
gRrfyXN9zFwQk3GcCVjPnsvHgyMygsSK9HmxZX9c3OxH4kOP0+XDzxm5tk/JOOZO
JpOqw80pAgMBAAECggEABVJm6O+VFr2GBy82Ru5VtSjU42CHLg3oFpXpybTf2XW3
JLYAtx0XWML4yJyddQuutx0HX//nZ24DWwnqJfk7/rv3orSmfh9NL1jeooKRptkH
wC8aHdJa1cdomSQ2yrVobm1Ay6dgqFl6bcHyCgqIVV0AsaSOjpXV75wKkNYcp28a
9adU3uu0YIiSXAwPjRsmfLjgnIIQ0kdmnCJXBfZRHMob3Kdp+WW8wEYbXR2Y+7Qk
xVxL6CSpl+96xU/e0wtBiGIjI/Yo9CGmUtQ+8rWgo2uDjBtOJHzPRWq3glL5vifr
/457hvy4teUvQkEZjiS45ScB+gNJkWRRYXiODC3mAQKBgQD147c29mN/AIikyDwW
n/g9L4hAEiVCdBbacAi7YEXE7aRTls0PKo9wHe8UZnMR25ygpLe3Ocyt9UJyMtA1
hRBQ6udy+HsQzHJSYUTeZ2hF9guV/nERL269sBKd3uvu9sjoASf1cBCb0cQq/W9P
9GrPu9vKeewXCv55HR1vrAkjKQKBgQC8G/dJx4HGCaogBN03VCKOHfMb+FKwcTM8
o6HeHCCBhhKiLF95wI3onUxxDyPeRcUY6cAJvXjIKnfIUwmvtYDd0rWjJoehfTIi
6kzxO5TV7j7dB9wGCk7hckTTJMA9cwWdamNKa/fQvvr2T/zqMTKK1VH/VAuXSqa8
/Q75KXSaAQKBgD6icOspPBL/2c7Pyq6gtSUw1G4tsRwKYj/mRfMlBOCzACbixP1W
BA+1ME9sAWfNH0q0aq/bo020qeZZFzxq1ssl6ysYLJEM4xok+NwGT0Ny92SP2dFc
2cKr9TINtN+8MVUX5kvY7gIjHussf4xIYGTmH4imIgOEo2nKhtMJaZJxAoGARGDq
GGC0dNeohvEdtPIZiY2rhLxU2pybomxc989DpbGl7luw7SIos6kSU3rMcAzuo6q8
yw2b7S860ZAV0lBpIrlom4DzBqnq4rRG+d3NsaJkKZakzXFL0TnneNVAKFY6cUlu
BGyQ7cmukoDrD/qIQ91dbDIR8JILYGGg4sPGBgECgYEAhWpCfXx4flSRSqUXTn34
0kfj5lqy5LyKSRGcH39VEfvNR4AFGWCFl9aYkGN2JoaXQvBlbn1V/C6SuPgA/eID
jioqKxvbS5M/1qAdSH1MfKDi2RYLGb3QkKbUPrbUduKz/6fx1/xAeGx0wtxcrrEj
Ep8xOYgvaFhoMZx6X+SniM8=
-----END PRIVATE KEY-----
";

/// Public half of [`RSA_PRIVATE_PEM`], used only to verify the OAuth assertion.
const RSA_PUBLIC_PEM: &str = r"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAtK4XFkMS/Yt/p5OUqm1k
3u8Uw/8USOJlSPS6NbK5m7ea3rdMt7b9PslZ8Rmv4z2eVXh/UNmtdQozGPOCVLGA
2XyGwvOLM2lSZCqAUumXuBRepoOP/ryAlvM8ZaA1by0Gfi0WBMOVq+fmkaEvbN8L
cOKEVm7cZQoqIbb8DhS4V+ZLvGJVm8X6fbeetMlIidKQRXUJxSqpDqmCfAM1WcPt
2FE2nRmN8wzKCYpY2MPkQF1UGvN0TjG3YmTElRZvvE9ht5XlJiD4tm9bFYEa38lz
fcxcEJNxnAlYz57Lx4MjMoLEivR5sWV/XNzsR+JDj9Plw88ZubZPyTjmTiaTqsPN
KQIDAQAB
-----END PUBLIC KEY-----
";

fn token_uri(mock: &MockUpstream) -> String {
    format!("{}/token", mock.base_url())
}

fn account(mock: &MockUpstream) -> FcmServiceAccount {
    FcmServiceAccount {
        project_id: PROJECT_ID.to_owned(),
        client_email: CLIENT_EMAIL.to_owned(),
        private_key_pem: RSA_PRIVATE_PEM.to_owned(),
        token_uri: token_uri(mock),
    }
}

fn sender(mock: &MockUpstream) -> FcmPushSender {
    let client = HttpClient::new("fcm-test", HttpClientOptions::new(mock.base_url()))
        .expect("mock client builds");
    FcmPushSender::new(account(mock), Arc::new(client)).expect("sender builds")
}

/// Queue a successful OAuth2 token response.
fn token_response(mock: &MockUpstream, access_token: &str) {
    mock.json(
        200,
        &json!({
            "access_token": access_token,
            "expires_in": 3600,
            "token_type": "Bearer",
        }),
    );
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

fn assertion_from_form(form: &str) -> &str {
    form.split("assertion=")
        .nth(1)
        .expect("assertion form member")
        .split('&')
        .next()
        .expect("assertion value")
}

fn verified_assertion(assertion: &str, expected_audience: &str) -> serde_json::Value {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[CLIENT_EMAIL]);
    validation.set_audience(&[expected_audience]);
    let key = DecodingKey::from_rsa_pem(RSA_PUBLIC_PEM.as_bytes()).expect("test public key");
    jsonwebtoken::decode::<serde_json::Value>(assertion, &key, &validation)
        .expect("OAuth assertion verifies")
        .claims
}

#[tokio::test]
async fn a_push_exchanges_the_service_account_assertion_and_sends_the_message() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.json(200, &json!({ "name": "projects/demo-project/messages/1" }));
    let message = PushMessage::alert(PushPlatform::Fcm, DEVICE_TOKEN, "Hello", "World")
        .with_data(json!({ "thread-id": "chat-1", "unread": 3 }))
        .with_collapse_key("chat")
        .with_ttl(Duration::from_secs(60));

    let outcome = sender(&mock).send(message).await.expect("push accepted");
    assert_eq!(outcome, PushOutcome::Accepted);

    let requests = mock.requests();
    assert_eq!(requests.len(), 2);

    // 1. OAuth2 token exchange.
    let token_request = &requests[0];
    assert_eq!(token_request.method, "POST");
    assert_eq!(token_request.path, "/token");
    assert_eq!(
        header(token_request, "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    let form = std::str::from_utf8(&token_request.body).expect("form is UTF-8");
    assert!(form.starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&"));
    let assertion = assertion_from_form(form);
    assert_eq!(
        jsonwebtoken::decode_header(assertion)
            .expect("assertion header")
            .alg,
        Algorithm::RS256
    );
    let claims = verified_assertion(assertion, &token_uri(&mock));
    assert_eq!(claims["iss"], CLIENT_EMAIL);
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/firebase.messaging"
    );
    let issued_at = claims["iat"].as_u64().expect("iat");
    assert_eq!(claims["exp"].as_u64(), Some(issued_at + 3600));

    // 2. messages:send.
    let send_request = &requests[1];
    assert_eq!(send_request.method, "POST");
    assert_eq!(send_request.path, MESSAGES_PATH);
    assert_eq!(bearer_token(send_request), "ya29.first");
    let body: serde_json::Value =
        serde_json::from_slice(&send_request.body).expect("message body is JSON");
    assert_eq!(body["message"]["token"], DEVICE_TOKEN);
    assert_eq!(body["message"]["notification"]["title"], "Hello");
    assert_eq!(body["message"]["notification"]["body"], "World");
    assert_eq!(body["message"]["data"]["thread-id"], "chat-1");
    // FCM data values are strings.
    assert_eq!(body["message"]["data"]["unread"], "3");
    assert_eq!(body["message"]["android"]["collapse_key"], "chat");
    assert_eq!(body["message"]["android"]["ttl"], "60s");
    assert_eq!(body["message"]["android"]["priority"], "high");
}

#[tokio::test]
async fn the_access_token_is_cached_between_sends() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.json(200, &json!({}));
    mock.json(200, &json!({}));
    let sender = sender(&mock);

    sender
        .send(PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN))
        .await
        .expect("first send");
    sender
        .send(PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN))
        .await
        .expect("second send");

    let requests = mock.requests();
    assert_eq!(requests.len(), 3, "one token exchange + two sends");
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/token")
            .count(),
        1
    );
    let send_requests: Vec<&RecordedRequest> = requests
        .iter()
        .filter(|request| request.path == MESSAGES_PATH)
        .collect();
    assert_eq!(send_requests.len(), 2);
    assert_eq!(bearer_token(send_requests[0]), "ya29.first");
    assert_eq!(bearer_token(send_requests[1]), "ya29.first");
}

#[tokio::test]
async fn concurrent_sends_share_one_access_token() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    for _ in 0..4 {
        mock.json(200, &json!({}));
    }
    let sender = Arc::new(sender(&mock));

    let mut tasks = Vec::new();
    for _ in 0..4 {
        let sender = Arc::clone(&sender);
        tasks.push(tokio::spawn(async move {
            sender
                .send(PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN))
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
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/token")
            .count(),
        1,
        "concurrent sends must share one token exchange"
    );
    let tokens: HashSet<&str> = requests
        .iter()
        .filter(|request| request.path == MESSAGES_PATH)
        .map(bearer_token)
        .collect();
    assert_eq!(tokens.len(), 1);
}

#[tokio::test]
async fn an_unregistered_token_is_token_invalid() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.json(
        400,
        &json!({
            "error": {
                "status": "INVALID_ARGUMENT",
                "message": "The registration token is not a valid FCM registration token",
                "details": [{
                    "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                    "errorCode": "UNREGISTERED"
                }]
            }
        }),
    );
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::TokenInvalid
    );
}

#[tokio::test]
async fn a_not_found_message_is_token_invalid() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.json(
        404,
        &json!({ "error": { "status": "NOT_FOUND", "message": "Requested entity was not found." } }),
    );
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::TokenInvalid
    );
}

#[tokio::test]
async fn a_non_token_bad_request_is_a_request_defect() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.json(
        400,
        &json!({
            "error": {
                "status": "FAILED_PRECONDITION",
                "message": format!("bad payload for {DEVICE_TOKEN}")
            }
        }),
    );
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("an invalid payload is a request defect");
    assert_eq!(error.status_code(), 500);
    assert!(!error.to_string().contains(DEVICE_TOKEN));
    assert!(error.public_detail().is_none());
}

#[tokio::test]
async fn rate_limiting_is_retryable_with_the_retry_after_header() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    script(&mock, 429, &[("retry-after", "5")], "{}");
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::Retryable {
            retry_after: Some(Duration::from_secs(5)),
        }
    );
}

#[tokio::test]
async fn an_unavailable_service_is_retryable_with_the_retry_info_delay() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.json(
        503,
        &json!({
            "error": {
                "status": "UNAVAILABLE",
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.RetryInfo",
                    "retryDelay": "2.5s"
                }]
            }
        }),
    );
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::Retryable {
            retry_after: Some(Duration::from_secs(3)),
        }
    );
}

#[tokio::test]
async fn a_server_error_is_retryable() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.status(500);
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    assert_eq!(
        sender(&mock).send(message).await.expect("verdict"),
        PushOutcome::Retryable { retry_after: None }
    );
}

#[tokio::test]
async fn a_rejected_access_token_clears_the_cache() {
    let mock = MockUpstream::start().await.expect("mock starts");
    token_response(&mock, "ya29.first");
    mock.status(401);
    token_response(&mock, "ya29.second");
    mock.json(200, &json!({}));
    let sender = sender(&mock);

    let rejected = sender
        .send(PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN))
        .await
        .expect_err("a rejected access token is a transport/configuration failure");
    assert_eq!(rejected.status_code(), 503);
    assert!(!rejected.to_string().contains("ya29.first"));

    let accepted = sender
        .send(PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN))
        .await
        .expect("second attempt");
    assert_eq!(accepted, PushOutcome::Accepted);

    let requests = mock.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/token")
            .count(),
        2,
        "the rejected token must be refreshed"
    );
    let send_requests: Vec<&RecordedRequest> = requests
        .iter()
        .filter(|request| request.path == MESSAGES_PATH)
        .collect();
    assert_eq!(bearer_token(send_requests[1]), "ya29.second");
}

#[tokio::test]
async fn a_platform_mismatch_is_a_validation_error_and_sends_nothing() {
    let mock = MockUpstream::start().await.expect("mock starts");
    // Valid APNs message: validation succeeds, the platform check must reject it.
    let message = PushMessage::silent(PushPlatform::Apns, DEVICE_TOKEN);
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
        .send(PushMessage::silent(PushPlatform::Fcm, ""))
        .await
        .expect_err("empty token is invalid");
    assert_eq!(error.code(), "validation_failed");
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn a_malformed_service_account_key_fails_at_construction() {
    let mock = MockUpstream::start().await.expect("mock starts");
    let client = HttpClient::new("fcm-test", HttpClientOptions::new(mock.base_url()))
        .expect("mock client builds");
    let account = FcmServiceAccount {
        project_id: PROJECT_ID.to_owned(),
        client_email: CLIENT_EMAIL.to_owned(),
        private_key_pem: "-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----\n"
            .to_owned(),
        token_uri: token_uri(&mock),
    };
    let error = FcmPushSender::new(account, Arc::new(client))
        .err()
        .expect("invalid PEM fails at construction");
    assert_eq!(error.status_code(), 500);
    assert!(error.public_detail().is_none());
    assert!(!error.to_string().contains("not-a-key"));
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn a_token_endpoint_rejection_is_a_configuration_error() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.json(400, &json!({ "error": "invalid_grant" }));
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("rejected service account");
    assert_eq!(error.status_code(), 500);
    assert!(error.public_detail().is_none());
    // No message request was attempted.
    assert!(mock
        .requests()
        .iter()
        .all(|request| request.path != MESSAGES_PATH));
}

#[tokio::test]
async fn a_token_endpoint_outage_is_retryable() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(503);
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("token endpoint outage");
    assert_eq!(error.status_code(), 503);
    assert!(error.retryable());
}

#[tokio::test]
async fn a_rate_limited_token_endpoint_is_retryable() {
    let mock = MockUpstream::start().await.expect("mock starts");
    mock.status(429);
    let message = PushMessage::silent(PushPlatform::Fcm, DEVICE_TOKEN);
    let error = sender(&mock)
        .send(message)
        .await
        .expect_err("token endpoint rate limited");
    assert_eq!(error.status_code(), 503);
    assert!(error.retryable());
}
