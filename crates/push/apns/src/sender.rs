//! APNs sender: request shape, provider-token cache and verdict mapping.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use webx_client::{HttpClientError, IHttpClient, OutboundRequest, OutboundResponse};
use webx_core::error::{Error, Result};
use webx_core::push::{IPushSender, PushMessage, PushOutcome, PushPlatform, PushPriority};

use crate::config::ApnsConfig;

/// Provider-token refresh interval.
///
/// APNs rejects provider tokens older than one hour and asks providers not to
/// mint them more often than every 20 minutes; 45 minutes sits safely inside
/// that window.
const PROVIDER_TOKEN_TTL: Duration = Duration::from_secs(45 * 60);

/// An APNs error body is a small JSON object (`{"reason": "..."}`).
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

/// A cached APNs provider token.
struct CachedProviderToken {
    value: String,
    refresh_at: Instant,
}

impl CachedProviderToken {
    fn is_fresh(&self, now: Instant) -> bool {
        now < self.refresh_at
    }
}

/// APNs delivery adapter.
///
/// Build one per APNs environment in the composition root and register it as
/// the singleton [`IPushSender`]. The injected
/// [`IHttpClient`](webx_client::IHttpClient) must target
/// [`ApnsEnvironment::base_url`](crate::ApnsEnvironment::base_url) and be built
/// with `http2_only: true`: APNs speaks HTTP/2 only.
pub struct ApnsPushSender {
    config: ApnsConfig,
    client: Arc<dyn IHttpClient>,
    signing_key: EncodingKey,
    provider_token: Mutex<Option<CachedProviderToken>>,
}

impl ApnsPushSender {
    /// Build a sender from `config` and an HTTP client for the APNs host.
    ///
    /// The `.p8` signing key is parsed here, so a malformed PEM fails fast as a
    /// configuration defect (`500`, fixed text) instead of failing every
    /// message. Key material is never copied into the error.
    pub fn new(config: ApnsConfig, client: Arc<dyn IHttpClient>) -> Result<Self> {
        config.validate()?;
        let signing_key =
            EncodingKey::from_ec_pem(config.private_key_pem.as_bytes()).map_err(|_| {
                Error::internal(
                "APNs auth key is not a valid PKCS#8 ES256 private key; check the .p8 signing key",
            )
            })?;
        Ok(Self {
            config,
            client,
            signing_key,
            provider_token: Mutex::new(None),
        })
    }

    /// Return a fresh provider token, minting one at most once per TTL window.
    ///
    /// The cache lock is held across minting (a purely local, synchronous
    /// operation), so concurrent sends observe a single refresh.
    async fn provider_token(&self) -> Result<String> {
        let mut cached = self.provider_token.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.is_fresh(Instant::now()) {
                return Ok(token.value.clone());
            }
        }
        let value = self.sign_provider_token()?;
        *cached = Some(CachedProviderToken {
            value: value.clone(),
            refresh_at: Instant::now() + PROVIDER_TOKEN_TTL,
        });
        tracing::debug!("minted a new APNs provider token");
        Ok(value)
    }

    /// ES256 provider token: `kid` header, `iss` = team id, `iat` = now.
    fn sign_provider_token(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            iat: u64,
        }

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.config.key_id.clone());
        let claims = Claims {
            iss: &self.config.team_id,
            iat: unix_secs(),
        };
        jsonwebtoken::encode(&header, &claims, &self.signing_key).map_err(|_| {
            Error::internal(
                "the APNs provider token could not be signed; check the .p8 signing key",
            )
        })
    }

    async fn clear_provider_token(&self) {
        *self.provider_token.lock().await = None;
    }

    /// Build the `/3/device/{token}` request, including the APNs headers and
    /// the `aps` payload.
    fn request(&self, message: &PushMessage, provider_token: &str) -> Result<OutboundRequest> {
        let payload = payload(message);
        let mut request =
            OutboundRequest::post(format!("/3/device/{}", encode_path_segment(&message.token)))
                .header("authorization", format!("Bearer {provider_token}"))
                .header("apns-topic", self.config.topic.as_str())
                .header("apns-push-type", push_type(message))
                .header("apns-priority", priority(message));
        if let Some(ttl) = message.ttl {
            request = request.header(
                "apns-expiration",
                unix_secs().saturating_add(ttl.as_secs()).to_string(),
            );
        }
        if let Some(collapse_key) = &message.collapse_key {
            request = request.header("apns-collapse-id", collapse_key.as_str());
        }
        request
            .json(&payload)
            .map_err(|_| Error::internal("the APNs payload could not be serialized"))
    }

    /// Map an APNs response to a provider verdict.
    ///
    /// `ExpiredProviderToken` clears the cached provider token so the next
    /// attempt mints a new one.
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

        let retry_after = headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let raw_body = body.collect(MAX_ERROR_BODY_BYTES).await.unwrap_or_default();
        let reason = provider_reason(&raw_body);

        match status {
            400 | 403 => match reason.as_deref() {
                Some(
                    "BadDeviceToken"
                    | "DeviceTokenNotForTopic"
                    | "MissingDeviceToken"
                    | "Unregistered",
                ) => Ok(PushOutcome::TokenInvalid),
                Some("ExpiredProviderToken") => {
                    self.clear_provider_token().await;
                    Ok(PushOutcome::Retryable { retry_after })
                }
                Some("InvalidProviderToken" | "MissingProviderToken") => Err(credentials_error()),
                Some(other) => Err(rejected_error(status, Some(other))),
                // A bare 403 is APNs rejecting the provider token itself.
                None if status == 403 => Err(credentials_error()),
                None => Err(rejected_error(status, None)),
            },
            401 => Err(credentials_error()),
            410 => Ok(PushOutcome::TokenInvalid),
            // 429 = rate limited / provider token updated too often; APNs
            // documents 500 and 502/503 as transient as well.
            429 | 500..=599 => Ok(PushOutcome::Retryable { retry_after }),
            _ => Err(rejected_error(status, reason.as_deref())),
        }
    }
}

#[async_trait]
impl IPushSender for ApnsPushSender {
    async fn send(&self, message: PushMessage) -> Result<PushOutcome> {
        message.validate()?;
        if message.platform != PushPlatform::Apns {
            return Err(Error::validation(
                "this sender delivers APNs pushes; the message targets a different platform",
            ));
        }
        let provider_token = self.provider_token().await?;
        let request = self.request(&message, &provider_token)?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(transport_error)?;
        self.classify(response).await
    }
}

/// Build the APNs payload: the `aps` dictionary plus flattened application data.
fn payload(message: &PushMessage) -> Value {
    let mut aps = Map::new();
    match &message.notification {
        Some(notification) => {
            let mut alert = Map::new();
            alert.insert(
                "title".to_owned(),
                Value::String(notification.title.clone()),
            );
            alert.insert("body".to_owned(), Value::String(notification.body.clone()));
            aps.insert("alert".to_owned(), Value::Object(alert));
            if let Some(badge) = notification.badge {
                aps.insert("badge".to_owned(), Value::from(badge));
            }
            if let Some(sound) = &notification.sound {
                aps.insert("sound".to_owned(), Value::String(sound.clone()));
            }
        }
        // A data-only push must be a background push; `content-available` wakes
        // the app without showing an alert.
        None => {
            aps.insert("content-available".to_owned(), Value::from(1));
        }
    }

    let mut body = Map::new();
    body.insert("aps".to_owned(), Value::Object(aps));
    if let Some(data) = message.data.as_object() {
        for (key, value) in data {
            // Apple owns `aps`; application data must not override it.
            if key == "aps" {
                continue;
            }
            body.insert(key.clone(), value.clone());
        }
    }
    Value::Object(body)
}

/// `apns-push-type`: an alert notification, or a silent background push.
fn push_type(message: &PushMessage) -> &'static str {
    if message.notification.is_some() {
        "alert"
    } else {
        "background"
    }
}

/// `apns-priority`: Apple requires `10` for user-visible alerts and `5` for
/// background-only pushes; an explicit high priority also selects `10`.
fn priority(message: &PushMessage) -> &'static str {
    if message.priority == PushPriority::High || message.notification.is_some() {
        "10"
    } else {
        "5"
    }
}

/// Extract the APNs `reason` from an error body.
///
/// APNs reasons are fixed ASCII identifiers. Anything else is untrusted server
/// text and is dropped rather than copied into an error message.
fn provider_reason(body: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        reason: Option<String>,
    }

    let reason = serde_json::from_slice::<ErrorBody>(body).ok()?.reason?;
    let safe = reason.len() <= 64
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    safe.then_some(reason)
}

/// Percent-encode one path segment so a malformed device token cannot escape
/// `/3/device/`.
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

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Credential rejection is a configuration defect: fixed text, retryable so
/// the application can rotate the key and retry.
fn credentials_error() -> Error {
    Error::unavailable(
        "APNs rejected the provider credentials; check the APNs team id, key id and signing key",
    )
}

/// A request-level rejection the adapter cannot fix: log-only diagnostic.
fn rejected_error(status: u16, reason: Option<&str>) -> Error {
    match reason {
        Some(reason) => Error::internal(format!(
            "APNs rejected the push request (HTTP {status}, reason {reason})"
        )),
        None => Error::internal(format!("APNs rejected the push request (HTTP {status})")),
    }
}

/// Never render the underlying client error: its messages can embed the request
/// URI, and the APNs path carries the device token.
fn transport_error(error: HttpClientError) -> Error {
    match error {
        HttpClientError::Timeout(_) => Error::unavailable("the APNs request timed out"),
        HttpClientError::Connect(_) => Error::unavailable("the APNs endpoint is unreachable"),
        _ => Error::internal("the APNs request could not be sent; check the client configuration"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::push::PushNotification;

    #[test]
    fn provider_tokens_are_cached_inside_apples_window() {
        assert!(PROVIDER_TOKEN_TTL >= Duration::from_secs(20 * 60));
        assert!(PROVIDER_TOKEN_TTL <= Duration::from_secs(60 * 60));

        let now = Instant::now();
        let token = CachedProviderToken {
            value: "jwt".to_owned(),
            refresh_at: now + PROVIDER_TOKEN_TTL,
        };
        assert!(token.is_fresh(now));
        assert!(token.is_fresh(now + PROVIDER_TOKEN_TTL - Duration::from_secs(1)));
        assert!(!token.is_fresh(now + PROVIDER_TOKEN_TTL));
    }

    #[test]
    fn an_alert_payload_carries_the_aps_dictionary_and_flat_data() {
        let message = PushMessage::alert(PushPlatform::Apns, "device", "Hello", "World").with_data(
            serde_json::json!({
                "thread-id": "chat-1",
                "aps": {"badge": 99},
            }),
        );
        let body = payload(&message);
        assert_eq!(body["aps"]["alert"]["title"], "Hello");
        assert_eq!(body["aps"]["alert"]["body"], "World");
        // Application data is flattened into the root object…
        assert_eq!(body["thread-id"], "chat-1");
        // …but cannot override the protocol-reserved `aps` dictionary.
        assert!(body["aps"]["badge"].is_null());
    }

    #[test]
    fn a_data_only_payload_is_a_background_push() {
        let message = PushMessage::silent(PushPlatform::Apns, "device").with_notification(
            PushNotification::new("t", "b")
                .with_badge(3)
                .with_sound("ping"),
        );
        let alert_payload = payload(&message);
        assert_eq!(alert_payload["aps"]["badge"], 3);
        assert_eq!(alert_payload["aps"]["sound"], "ping");
        assert_eq!(push_type(&message), "alert");
        assert_eq!(priority(&message), "10");

        let silent = PushMessage::silent(PushPlatform::Apns, "device");
        let silent_payload = payload(&silent);
        assert_eq!(silent_payload["aps"]["content-available"], 1);
        assert_eq!(push_type(&silent), "background");
        assert_eq!(priority(&silent), "5");
    }

    #[test]
    fn a_high_priority_data_push_uses_priority_ten() {
        let message =
            PushMessage::silent(PushPlatform::Apns, "device").with_priority(PushPriority::High);
        assert_eq!(priority(&message), "10");
        assert_eq!(push_type(&message), "background");
    }

    #[test]
    fn a_device_token_cannot_escape_the_device_path_segment() {
        assert_eq!(encode_path_segment("abcDEF0123"), "abcDEF0123");
        assert_eq!(encode_path_segment("a/b c"), "a%2Fb%20c");
        assert_eq!(encode_path_segment(".."), "..");
    }

    #[test]
    fn only_apns_reason_identifiers_are_kept() {
        assert_eq!(
            provider_reason(br#"{"reason":"BadDeviceToken"}"#).as_deref(),
            Some("BadDeviceToken")
        );
        assert_eq!(provider_reason(br#"{"reason":"token leak"}"#), None);
        assert_eq!(provider_reason(b"not json"), None);
        assert_eq!(provider_reason(b""), None);
    }
}
