//! FCM sender: OAuth2 access-token cache, message shape and verdict mapping.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use webx_client::{HttpClientError, IHttpClient, OutboundRequest, OutboundResponse};
use webx_core::error::{Error, Result};
use webx_core::push::{IPushSender, PushMessage, PushOutcome, PushPlatform, PushPriority};

use crate::account::FcmServiceAccount;

/// OAuth2 scope required by the FCM HTTP v1 API.
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

/// JWT-bearer grant type for the OAuth2 token exchange.
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// Google access tokens are valid for one hour; refresh a minute before expiry.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Access-token and error bodies are small JSON documents.
const MAX_TOKEN_BODY_BYTES: usize = 16 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;

/// A cached FCM OAuth2 access token.
struct CachedAccessToken {
    value: String,
    refresh_at: Instant,
}

impl CachedAccessToken {
    fn is_fresh(&self, now: Instant) -> bool {
        now < self.refresh_at
    }
}

/// FCM HTTP v1 delivery adapter.
///
/// Build one in the composition root and register it as the singleton
/// [`IPushSender`]. The injected [`IHttpClient`](webx_client::IHttpClient) must
/// be built with the base URL `https://fcm.googleapis.com`, because
/// `messages:send` is posted as a path relative to that origin (the OAuth2 token
/// endpoint is absolute and comes from the service-account JSON).
pub struct FcmPushSender {
    project_id: String,
    client_email: String,
    token_uri: String,
    client: Arc<dyn IHttpClient>,
    signing_key: EncodingKey,
    access_token: Mutex<Option<CachedAccessToken>>,
}

impl FcmPushSender {
    /// Build a sender from the service-account credentials and an HTTP client.
    ///
    /// The service-account key is parsed here, so malformed credentials fail
    /// fast as a configuration defect (`500`, fixed text) instead of failing
    /// every message. The private key never appears in the error.
    pub fn new(account: FcmServiceAccount, client: Arc<dyn IHttpClient>) -> Result<Self> {
        let account = account.normalize()?;
        let signing_key = EncodingKey::from_rsa_pem(account.private_key_pem.as_bytes()).map_err(
            |_| {
                Error::internal(
                    "the FCM service-account key is not a valid PKCS#8 RSA private key; check the service-account JSON",
                )
            },
        )?;
        Ok(Self {
            project_id: account.project_id,
            client_email: account.client_email,
            token_uri: account.token_uri,
            client,
            signing_key,
            access_token: Mutex::new(None),
        })
    }

    /// Return a fresh access token, exchanging the OAuth2 assertion at most once
    /// per validity window.
    ///
    /// The cache lock is held across the token exchange, so concurrent sends
    /// wait and then re-check the cache instead of each requesting a token
    /// (single-flight).
    async fn access_token(&self) -> Result<String> {
        let mut cached = self.access_token.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.is_fresh(Instant::now()) {
                return Ok(token.value.clone());
            }
        }
        let (value, ttl) = self.fetch_access_token().await?;
        *cached = Some(CachedAccessToken {
            value: value.clone(),
            refresh_at: Instant::now() + ttl,
        });
        tracing::debug!("refreshed the FCM OAuth2 access token");
        Ok(value)
    }

    async fn clear_access_token(&self) {
        *self.access_token.lock().await = None;
    }

    /// RS256 assertion signed by the service account.
    fn assertion(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            scope: &'static str,
            aud: &'a str,
            iat: u64,
            exp: u64,
        }

        let issued_at = unix_secs();
        let claims = Claims {
            iss: &self.client_email,
            scope: FCM_SCOPE,
            aud: &self.token_uri,
            iat: issued_at,
            exp: issued_at.saturating_add(3600),
        };
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.signing_key).map_err(
            |_| {
                Error::internal(
                    "the FCM service-account assertion could not be signed; check the service-account key",
                )
            },
        )
    }

    /// Exchange the signed assertion for an access token.
    async fn fetch_access_token(&self) -> Result<(String, Duration)> {
        let assertion = self.assertion()?;
        let form = format!(
            "grant_type={}&assertion={}",
            encode_form_component(GRANT_TYPE),
            encode_form_component(&assertion),
        );
        let request = OutboundRequest::post(self.token_uri.as_str())
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form);
        let response = self
            .client
            .execute(request)
            .await
            .map_err(transport_error)?;
        let OutboundResponse { status, body, .. } = response;
        if !(200..300).contains(&status) {
            return Err(token_endpoint_error(status));
        }
        let raw = body
            .collect(MAX_TOKEN_BODY_BYTES)
            .await
            .map_err(|_| Error::internal("the FCM token response body could not be read"))?;
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            #[serde(default)]
            expires_in: Option<u64>,
        }

        let token: TokenResponse = serde_json::from_slice(&raw)
            .map_err(|_| Error::internal("the FCM token endpoint returned an invalid JSON body"))?;
        if token.access_token.trim().is_empty() {
            return Err(Error::internal(
                "the FCM token endpoint returned an empty access token",
            ));
        }
        Ok((token.access_token, token_cache_ttl(token.expires_in)))
    }

    /// Build the `messages:send` request for `message`.
    fn request(&self, message: &PushMessage, access_token: &str) -> Result<OutboundRequest> {
        let body = message_body(message);
        OutboundRequest::post(self.messages_path())
            .header("authorization", format!("Bearer {access_token}"))
            .json(&body)
            .map_err(|_| Error::internal("the FCM message payload could not be serialized"))
    }

    fn messages_path(&self) -> String {
        format!(
            "/v1/projects/{}/messages:send",
            encode_path_segment(&self.project_id)
        )
    }

    /// Map an FCM response to a provider verdict.
    ///
    /// `401`/`403` clear the cached access token so the next attempt obtains a
    /// fresh one.
    async fn classify(&self, response: OutboundResponse) -> Result<PushOutcome> {
        let OutboundResponse {
            status,
            headers,
            body,
            ..
        } = response;
        if (200..300).contains(&status) {
            return Ok(PushOutcome::Accepted);
        }

        let retry_after_header = headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let raw_body = body.collect(MAX_ERROR_BODY_BYTES).await.unwrap_or_default();
        let error = serde_json::from_slice::<FcmErrorEnvelope>(&raw_body)
            .ok()
            .map(|envelope| envelope.error);

        match status {
            401 | 403 => {
                self.clear_access_token().await;
                Err(Error::unavailable(
                    "FCM rejected the OAuth2 access token; retry after it is refreshed",
                ))
            }
            429 | 500..=599 => {
                let retry_info = error.as_ref().and_then(retry_info);
                Ok(PushOutcome::Retryable {
                    retry_after: retry_after_header.or(retry_info),
                })
            }
            400 | 404 => match error.as_ref() {
                // FCM reports a dead registration token either as a top-level
                // status or as an `UNREGISTERED` detail.
                Some(error) if token_is_invalid(error) => Ok(PushOutcome::TokenInvalid),
                Some(error) => Err(rejected_error(status, safe_code(&error.status))),
                None => Err(rejected_error(status, None)),
            },
            _ => Err(rejected_error(status, None)),
        }
    }
}

#[async_trait]
impl IPushSender for FcmPushSender {
    async fn send(&self, message: PushMessage) -> Result<PushOutcome> {
        message.validate()?;
        if message.platform != PushPlatform::Fcm {
            return Err(Error::validation(
                "this sender delivers FCM pushes; the message targets a different platform",
            ));
        }
        let access_token = self.access_token().await?;
        let request = self.request(&message, &access_token)?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(transport_error)?;
        self.classify(response).await
    }
}

/// Build the FCM HTTP v1 request body (`{"message": {...}}`).
fn message_body(message: &PushMessage) -> Value {
    let mut payload = Map::new();
    payload.insert("token".to_owned(), Value::String(message.token.clone()));
    if let Some(notification) = &message.notification {
        let mut block = Map::new();
        block.insert(
            "title".to_owned(),
            Value::String(notification.title.clone()),
        );
        block.insert("body".to_owned(), Value::String(notification.body.clone()));
        payload.insert("notification".to_owned(), Value::Object(block));
    }
    if let Some(data) = message.data.as_object() {
        if !data.is_empty() {
            // FCM only accepts string values in `data`; nested JSON is
            // serialized to its JSON text so nothing is lost silently.
            let mut strings = Map::new();
            for (key, value) in data {
                strings.insert(key.clone(), Value::String(data_value(value)));
            }
            payload.insert("data".to_owned(), Value::Object(strings));
        }
    }

    let mut android = Map::new();
    if let Some(collapse_key) = &message.collapse_key {
        android.insert(
            "collapse_key".to_owned(),
            Value::String(collapse_key.clone()),
        );
    }
    if let Some(ttl) = message.ttl {
        android.insert(
            "ttl".to_owned(),
            Value::String(format!("{}s", ttl.as_secs())),
        );
    }
    android.insert(
        "priority".to_owned(),
        Value::String(android_priority(message).to_owned()),
    );
    payload.insert("android".to_owned(), Value::Object(android));

    let mut root = Map::new();
    root.insert("message".to_owned(), Value::Object(payload));
    Value::Object(root)
}

/// FCM `data` values must be strings; non-string JSON values keep their JSON
/// text (numbers, booleans, nested objects and arrays).
fn data_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Android delivery priority: `high` for user-visible alerts, `normal`
/// otherwise (an explicit high priority also selects `high`).
fn android_priority(message: &PushMessage) -> &'static str {
    if message.priority == PushPriority::High || message.notification.is_some() {
        "high"
    } else {
        "normal"
    }
}

/// Access-token cache lifetime: `expires_in` minus a one-minute safety margin,
/// never below one second.
fn token_cache_ttl(expires_in: Option<u64>) -> Duration {
    let declared = expires_in.unwrap_or(3600);
    Duration::from_secs(declared)
        .saturating_sub(TOKEN_REFRESH_MARGIN)
        .max(Duration::from_secs(1))
}

/// The FCM error envelope (`{"error": {"status": ..., "details": [...]}}`).
#[derive(Deserialize)]
struct FcmErrorEnvelope {
    error: FcmErrorBody,
}

#[derive(Deserialize)]
struct FcmErrorBody {
    #[serde(default)]
    status: String,
    #[serde(default)]
    details: Vec<FcmErrorDetail>,
}

#[derive(Deserialize)]
struct FcmErrorDetail {
    #[serde(default, rename = "errorCode")]
    error_code: Option<String>,
    #[serde(default, rename = "retryDelay")]
    retry_delay: Option<String>,
}

/// Whether the error reports a dead/unknown registration token.
fn token_is_invalid(error: &FcmErrorBody) -> bool {
    matches!(error.status.as_str(), "INVALID_ARGUMENT" | "NOT_FOUND")
        || error
            .details
            .iter()
            .any(|detail| detail.error_code.as_deref() == Some("UNREGISTERED"))
}

/// Extract a retry hint from a `google.rpc.RetryInfo` detail.
fn retry_info(error: &FcmErrorBody) -> Option<Duration> {
    error
        .details
        .iter()
        .find_map(|detail| detail.retry_delay.as_deref().and_then(parse_retry_delay))
}

/// Parse a `google.rpc.RetryInfo.retryDelay` value such as `"1.5s"`.
fn parse_retry_delay(value: &str) -> Option<Duration> {
    let seconds = value.trim().strip_suffix('s')?.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    // One day is far longer than any useful push retry; cap absurd hints.
    let seconds = seconds.ceil().min(86_400.0);
    Some(Duration::from_secs_f64(seconds))
}

/// Keep only provider error identifiers (fixed ASCII tokens); never copy
/// arbitrary server text into a diagnostic.
fn safe_code(value: &str) -> Option<&str> {
    let safe = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    safe.then_some(value)
}

/// Percent-encode one path segment so a malformed project id cannot escape
/// `/v1/projects/`.
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Percent-encode one `application/x-www-form-urlencoded` component.
fn encode_form_component(value: &str) -> String {
    encode_path_segment(value)
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// A request-level rejection the adapter cannot fix: log-only diagnostic with a
/// sanitized provider status, never the provider message (it can echo tokens).
fn rejected_error(status: u16, provider_status: Option<&str>) -> Error {
    match provider_status {
        Some(provider_status) => Error::internal(format!(
            "FCM rejected the message (HTTP {status}, status {provider_status})"
        )),
        None => Error::internal(format!("FCM rejected the message (HTTP {status})")),
    }
}

/// Credential/token-endpoint failures are configuration defects; `429` and
/// `5xx` from the token endpoint are transient.
fn token_endpoint_error(status: u16) -> Error {
    if status == 429 || status >= 500 {
        Error::unavailable("the FCM OAuth2 token endpoint is unavailable")
    } else {
        Error::internal(format!(
            "the FCM service account was rejected by the OAuth2 token endpoint (HTTP {status})"
        ))
    }
}

/// Never render the underlying client error: its messages can embed request
/// URLs and headers.
fn transport_error(error: HttpClientError) -> Error {
    match error {
        HttpClientError::Timeout(_) => Error::unavailable("the FCM request timed out"),
        HttpClientError::Connect(_) => Error::unavailable("the FCM endpoint is unreachable"),
        _ => Error::internal("the FCM request could not be sent; check the client configuration"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::push::PushNotification;

    #[test]
    fn the_access_token_is_cached_a_minute_before_it_expires() {
        assert_eq!(token_cache_ttl(Some(3600)), Duration::from_secs(3540));
        assert_eq!(token_cache_ttl(None), Duration::from_secs(3540));
        // A token endpoint that claims a shorter life still gets a usable cache.
        assert_eq!(token_cache_ttl(Some(30)), Duration::from_secs(1));
    }

    #[test]
    fn access_tokens_are_fresh_until_the_refresh_deadline() {
        let now = Instant::now();
        let token = CachedAccessToken {
            value: "ya29.token".to_owned(),
            refresh_at: now + Duration::from_secs(3540),
        };
        assert!(token.is_fresh(now));
        assert!(!token.is_fresh(now + Duration::from_secs(3540)));
    }

    #[test]
    fn the_message_body_carries_token_notification_data_and_android_options() {
        let message = PushMessage::alert(PushPlatform::Fcm, "device-token", "Hello", "World")
            .with_data(serde_json::json!({
                "thread-id": "chat-1",
                "unread": 3,
                "muted": false,
            }))
            .with_collapse_key("chat")
            .with_ttl(Duration::from_secs(60));
        let body = message_body(&message);
        let inner = &body["message"];
        assert_eq!(inner["token"], "device-token");
        assert_eq!(inner["notification"]["title"], "Hello");
        assert_eq!(inner["notification"]["body"], "World");
        // Every `data` value is a string, non-strings keep their JSON text.
        assert_eq!(inner["data"]["thread-id"], "chat-1");
        assert_eq!(inner["data"]["unread"], "3");
        assert_eq!(inner["data"]["muted"], "false");
        assert_eq!(inner["android"]["collapse_key"], "chat");
        assert_eq!(inner["android"]["ttl"], "60s");
        assert_eq!(inner["android"]["priority"], "high");
    }

    #[test]
    fn a_silent_data_push_uses_normal_android_priority() {
        let message = PushMessage::silent(PushPlatform::Fcm, "device-token");
        let body = message_body(&message);
        assert_eq!(body["message"]["android"]["priority"], "normal");
        assert!(body["message"]["notification"].is_null());
        assert!(body["message"]["data"].is_null());
        assert!(body["message"]["android"]["ttl"].is_null());
    }

    #[test]
    fn dead_tokens_are_recognized_by_status_or_unregistered_detail() {
        let by_status = FcmErrorBody {
            status: "NOT_FOUND".to_owned(),
            details: Vec::new(),
        };
        assert!(token_is_invalid(&by_status));
        let by_detail = FcmErrorBody {
            status: "FAILED_PRECONDITION".to_owned(),
            details: vec![FcmErrorDetail {
                error_code: Some("UNREGISTERED".to_owned()),
                retry_delay: None,
            }],
        };
        assert!(token_is_invalid(&by_detail));
        let unrelated = FcmErrorBody {
            status: "PERMISSION_DENIED".to_owned(),
            details: Vec::new(),
        };
        assert!(!token_is_invalid(&unrelated));
    }

    #[test]
    fn retry_info_is_parsed_from_a_duration_string_and_capped() {
        assert_eq!(parse_retry_delay("1.5s"), Some(Duration::from_secs(2)));
        assert_eq!(parse_retry_delay("3s"), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_delay("3"), None);
        assert_eq!(parse_retry_delay("-1s"), None);
        assert_eq!(
            parse_retry_delay("86400000s"),
            Some(Duration::from_secs(86_400))
        );

        let error = FcmErrorBody {
            status: "UNAVAILABLE".to_owned(),
            details: vec![FcmErrorDetail {
                error_code: None,
                retry_delay: Some("2s".to_owned()),
            }],
        };
        assert_eq!(retry_info(&error), Some(Duration::from_secs(2)));
    }

    #[test]
    fn provider_error_identifiers_are_sanitized() {
        assert_eq!(safe_code("INVALID_ARGUMENT"), Some("INVALID_ARGUMENT"));
        assert_eq!(safe_code("bad status"), None);
        assert_eq!(safe_code(""), None);
        assert_eq!(safe_code(&"a".repeat(65)), None);
    }

    #[test]
    fn the_project_path_segment_is_percent_encoded_and_form_values_survive() {
        assert_eq!(encode_path_segment("demo-project"), "demo-project");
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(
            encode_form_component(GRANT_TYPE),
            "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer"
        );
        // A JWT only uses base64url characters plus `.`, which are unreserved.
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiJ4In0.c2ln";
        assert_eq!(encode_form_component(jwt), jwt);
    }

    #[test]
    fn a_notification_with_a_badge_is_still_a_notification_block() {
        let message = PushMessage::silent(PushPlatform::Fcm, "device-token")
            .with_notification(PushNotification::new("Title", "Body"));
        let body = message_body(&message);
        assert_eq!(body["message"]["notification"]["title"], "Title");
        assert_eq!(body["message"]["android"]["priority"], "high");
    }
}
