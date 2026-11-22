//! Square REST transport: request construction, auth headers and error decoding.
//!
//! Every Square call is JSON over HTTPS with `Authorization: Bearer <token>`
//! and a pinned `Square-Version`. Requests are built as a plain
//! [`SquareRequest`] value first so header/body construction is unit-testable
//! without opening a socket; only [`execute`] touches the framework HTTP client.
//!
//! # Idempotency
//!
//! Square takes the idempotency key in the **JSON request body**
//! (`"idempotency_key"`), not in an `Idempotency-Key` HTTP header — the official
//! SDKs do the same. The adapter derives a stable key from the merchant order /
//! refund id when the caller does not supply one and truncates it to Square's
//! 45-character limit.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use webx_client::{IHttpClient, OutboundRequest};
use webx_pay::{PaymentError, ProviderId};

use crate::provider::SquareAccount;

/// Provider id carried by every error value.
const PROVIDER: ProviderId = ProviderId::Square;

/// Largest response body read from Square (bounded JSON).
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Square limits idempotency keys to 45 characters.
pub const IDEMPOTENCY_KEY_MAX: usize = 45;

/// HTTP method used by Square's REST API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    /// Query an object.
    Get,
    /// Create/complete/cancel an object.
    Post,
}

/// One Square API call before it is adapted to the framework HTTP client.
#[derive(Debug)]
pub(crate) struct SquareRequest {
    pub(crate) verb: Verb,
    pub(crate) path: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<Bytes>,
}

/// Build a request for `account`: bearer auth, pinned `Square-Version`, and a
/// JSON body with the matching `Content-Type`.
pub(crate) fn build_request(
    account: &SquareAccount,
    verb: Verb,
    path: impl Into<String>,
    body: Option<&Value>,
) -> Result<SquareRequest, PaymentError> {
    let mut headers = vec![
        (
            "authorization".to_string(),
            format!("Bearer {}", account.access_token),
        ),
        ("square-version".to_string(), account.api_version.clone()),
        ("accept".to_string(), "application/json".to_string()),
    ];
    let body = match body {
        Some(value) => {
            headers.push(("content-type".to_string(), "application/json".to_string()));
            let encoded = serde_json::to_vec(value).map_err(|error| {
                PaymentError::Serialization(format!("failed to encode square request: {error}"))
            })?;
            Some(Bytes::from(encoded))
        }
        None => None,
    };
    Ok(SquareRequest {
        verb,
        path: path.into(),
        headers,
        body,
    })
}

/// Send `request` to `account`'s environment and decode the JSON response.
///
/// Non-2xx responses are mapped to typed [`PaymentError`] values; transport
/// failures stay [`PaymentError::Transport`].
pub(crate) async fn execute(
    client: &Arc<dyn IHttpClient>,
    account: &SquareAccount,
    request: SquareRequest,
) -> Result<Value, PaymentError> {
    let url = format!("{}{}", account.base_url.trim_end_matches('/'), request.path);
    let mut outbound = match request.verb {
        Verb::Get => OutboundRequest::get(url),
        Verb::Post => OutboundRequest::post(url),
    };
    for (name, value) in request.headers {
        outbound = outbound.header(name, value);
    }
    if let Some(body) = request.body {
        outbound = outbound.body(body);
    }

    let response = client.execute(outbound).await?;
    let status = response.status;
    let request_id = response
        .headers
        .get("request-id")
        .or_else(|| response.headers.get("square-request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.body.collect(MAX_BODY_BYTES).await?;

    if (200..300).contains(&status) {
        if body.is_empty() {
            return Ok(json!({}));
        }
        return serde_json::from_slice(&body).map_err(|error| {
            PaymentError::Serialization(format!("invalid square response: {error}"))
        });
    }
    // Error bodies are `{ "errors": [{ category, code, detail }] }`; a
    // non-JSON body (proxy HTML, gateway page) must not mask the HTTP status.
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Err(map_error(status, &value, request_id))
}

/// Map a non-2xx Square response to a typed payment error.
///
/// * `404` → [`PaymentError::NotFound`].
/// * `401`/`403` → a non-retryable provider error: the adapter's credentials are
///   wrong, so core's fail-closed mapping surfaces HTTP `502` to the caller
///   instead of blaming the end user.
/// * `429` and `5xx` → retryable provider error.
/// * everything else → provider error with the Square category/code.
pub(crate) fn map_error(status: u16, value: &Value, request_id: Option<String>) -> PaymentError {
    let first = value
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first());
    let category = first
        .and_then(|error| error.get("category"))
        .and_then(Value::as_str);
    let code = first
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let detail = first
        .and_then(|error| error.get("detail"))
        .and_then(Value::as_str);
    let message = match (category, detail) {
        (Some(category), Some(detail)) => format!("{category}: {detail}"),
        (Some(category), None) => category.to_string(),
        (None, Some(detail)) => detail.to_string(),
        (None, None) => format!("square request failed with HTTP {status}"),
    };

    if status == 404 {
        return PaymentError::not_found(PROVIDER, message);
    }
    if status == 401 || status == 403 {
        return PaymentError::provider(
            PROVIDER,
            code,
            format!("square rejected the configured credentials: {message}"),
            Some(status),
            request_id,
            false,
        );
    }
    let retryable = status == 429 || (500..=599).contains(&status);
    PaymentError::provider(PROVIDER, code, message, Some(status), request_id, retryable)
}

/// Normalize an idempotency key to Square's 45-character limit.
///
/// Returns `preferred` when set, otherwise `fallback` (merchant order / refund
/// id). Longer values are replaced by a deterministic SHA-256 prefix so that a
/// retry of the same logical operation reuses the same key.
pub fn derive_idempotency_key(preferred: Option<&str>, fallback: &str) -> String {
    let candidate = preferred
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| fallback.trim());
    if candidate.len() <= IDEMPOTENCY_KEY_MAX {
        return candidate.to_string();
    }
    let digest = Sha256::digest(candidate.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex.truncate(IDEMPOTENCY_KEY_MAX);
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_pay::AccountId;

    fn account() -> SquareAccount {
        SquareAccount {
            id: AccountId::from("default"),
            access_token: "test-access-token".to_string(),
            signature_key: Some("test-signature-key".to_string()),
            location_id: Some("TESTLOCATION1".to_string()),
            application_id: None,
            live: false,
            api_version: "2025-09-24".to_string(),
            base_url: "https://connect.squareupsandbox.com".to_string(),
        }
    }

    #[test]
    fn post_requests_carry_auth_version_and_json() {
        let body = json!({ "idempotency_key": "key-1", "amount_money": { "amount": 100 } });
        let request = build_request(&account(), Verb::Post, "/v2/payments", Some(&body)).unwrap();
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        };
        assert_eq!(request.verb, Verb::Post);
        assert_eq!(request.path, "/v2/payments");
        assert_eq!(
            header("authorization").as_deref(),
            Some("Bearer test-access-token")
        );
        assert_eq!(header("square-version").as_deref(), Some("2025-09-24"));
        assert_eq!(header("content-type").as_deref(), Some("application/json"));
        let encoded: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        assert_eq!(encoded, body);
    }

    #[test]
    fn get_requests_have_no_body_or_content_type() {
        let request = build_request(&account(), Verb::Get, "/v2/payments/id-1", None).unwrap();
        assert_eq!(request.verb, Verb::Get);
        assert!(request.body.is_none());
        assert!(request
            .headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("content-type")));
    }

    #[test]
    fn idempotency_keys_are_derived_and_bounded() {
        assert_eq!(derive_idempotency_key(None, "ORDER-1"), "ORDER-1");
        assert_eq!(
            derive_idempotency_key(Some("  idem-1  "), "ORDER-1"),
            "idem-1"
        );
        assert_eq!(derive_idempotency_key(Some("   "), "ORDER-1"), "ORDER-1");
        let long = "x".repeat(200);
        let key = derive_idempotency_key(None, &long);
        assert_eq!(key.len(), IDEMPOTENCY_KEY_MAX);
        assert_eq!(key, derive_idempotency_key(None, &long));
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn maps_square_error_bodies() {
        let body = json!({
            "errors": [{
                "category": "INVALID_REQUEST_ERROR",
                "code": "MISSING_REQUIRED_PARAMETER",
                "detail": "Missing required parameter `source_id`."
            }]
        });
        let error = map_error(400, &body, None);
        assert!(matches!(
            error,
            PaymentError::Provider {
                http_status: Some(400),
                retryable: false,
                ..
            }
        ));
        assert_eq!(error.http_status(), 400);

        let auth = map_error(401, &body, None);
        // Credential failures are adapter configuration problems, not caller 401s.
        assert_eq!(auth.http_status(), 502);
        assert!(!auth.retryable());
        assert!(auth
            .to_string()
            .contains("rejected the configured credentials"));

        let missing = map_error(404, &json!({ "errors": [] }), None);
        assert_eq!(missing.http_status(), 404);

        let limited = map_error(429, &json!({}), None);
        assert!(limited.retryable());
        assert_eq!(limited.http_status(), 503);

        let outage = map_error(503, &json!({}), None);
        assert!(outage.retryable());
        assert_eq!(outage.http_status(), 502);
    }

    #[test]
    fn maps_non_json_error_bodies_without_losing_the_status() {
        let error = map_error(502, &Value::Null, None);
        assert_eq!(error.http_status(), 502);
        assert!(error.retryable());
        assert!(error.to_string().contains("HTTP 502"));
    }
}
