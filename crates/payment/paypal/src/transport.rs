//! PayPal HTTP transport: bearer token, JSON requests, error mapping.
//!
//! All outbound calls go through `rust-webx-client`'s [`IHttpClient`] port. The
//! adapter sends absolute per-account URLs so live and sandbox accounts can
//! share one pooled client.

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use serde_json::{json, Value};
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_pay::PaymentError;

use crate::auth::{basic_auth_header, TokenResponse};
use crate::options::{PayPalAccount, PayPalOptions};
use crate::PROVIDER;

/// Upper bound for a PayPal response body (they are small JSON documents).
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Verb {
    Get,
    Post,
}

/// Successful PayPal response with the support debug id.
pub(crate) struct PayPalResponse {
    pub(crate) value: Value,
    pub(crate) debug_id: Option<String>,
}

/// HTTP transport shared by the provider and the webhook verifier.
pub(crate) struct PayPalTransport {
    injected_client: Option<Arc<dyn IHttpClient>>,
    client: OnceLock<Result<Arc<dyn IHttpClient>, String>>,
    http_options: HttpClientOptions,
}

impl PayPalTransport {
    pub(crate) fn new(
        options: &PayPalOptions,
        injected_client: Option<Arc<dyn IHttpClient>>,
    ) -> Self {
        Self {
            injected_client,
            client: OnceLock::new(),
            http_options: options.http_options(options.client_base_url()),
        }
    }

    fn client(&self) -> Result<Arc<dyn IHttpClient>, PaymentError> {
        if let Some(client) = &self.injected_client {
            return Ok(Arc::clone(client));
        }
        let built = self.client.get_or_init(|| {
            HttpClient::new("paypal", self.http_options.clone())
                .map(|client| Arc::new(client) as Arc<dyn IHttpClient>)
                .map_err(|error| error.to_string())
        });
        built
            .as_ref()
            .map(Arc::clone)
            .map_err(|error| PaymentError::config(PROVIDER, error.clone()))
    }

    /// Validate that the pooled HTTP client can be built (startup/health probe).
    pub(crate) fn validate(&self) -> Result<(), PaymentError> {
        self.client().map(|_| ())
    }

    /// Send a JSON request and return the parsed success body.
    ///
    /// `PayPal-Request-Id` is attached when `request_id` is set (PayPal stores
    /// idempotency keys for 6 hours); `Prefer: return=representation` asks for a
    /// full resource instead of PayPal's minimal `{id, status, links}` response.
    pub(crate) async fn send(
        &self,
        account: &PayPalAccount,
        verb: Verb,
        path: &str,
        body: Option<&Value>,
        request_id: Option<&str>,
        prefer_representation: bool,
    ) -> Result<PayPalResponse, PaymentError> {
        let token = self.bearer_token(account).await?;
        let client = self.client()?;
        let url = account.url(path);
        let mut request = match verb {
            Verb::Get => OutboundRequest::get(url),
            Verb::Post => OutboundRequest::post(url),
        };
        request = request
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json");
        if let Some(request_id) = request_id {
            request = request.header("paypal-request-id", request_id.to_string());
        }
        if prefer_representation {
            request = request.header("prefer", "return=representation");
        }
        if let Some(body) = body {
            let encoded = serde_json::to_vec(body).map_err(|error| {
                PaymentError::Serialization(format!("cannot encode paypal request body: {error}"))
            })?;
            request = request
                .header("content-type", "application/json")
                .body(Bytes::from(encoded));
        }

        let response = client.execute(request.timeout(account.timeout)).await?;
        let status = response.status;
        let header_debug_id = response
            .headers
            .get("paypal-debug-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body.collect(MAX_BODY_BYTES).await?;
        if !(200..300).contains(&status) {
            return Err(map_api_error(status, &raw, header_debug_id));
        }
        let value = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).map_err(|error| {
                PaymentError::Serialization(format!("invalid paypal response JSON: {error}"))
            })?
        };
        Ok(PayPalResponse {
            value,
            debug_id: header_debug_id,
        })
    }

    /// Cached access token for `account`, refreshing through the OAuth2 token
    /// endpoint on first use/expiry.
    async fn bearer_token(&self, account: &PayPalAccount) -> Result<String, PaymentError> {
        account
            .tokens
            .token_or_refresh(self.fetch_token(account))
            .await
    }

    async fn fetch_token(&self, account: &PayPalAccount) -> Result<TokenResponse, PaymentError> {
        let client = self.client()?;
        let request = OutboundRequest::post(account.url("/v1/oauth2/token"))
            .header(
                "authorization",
                basic_auth_header(&account.client_id, &account.client_secret),
            )
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Bytes::from_static(b"grant_type=client_credentials"))
            .timeout(account.timeout);
        let response = client.execute(request).await?;
        let status = response.status;
        let header_debug_id = response
            .headers
            .get("paypal-debug-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body.collect(MAX_BODY_BYTES).await?;
        if !(200..300).contains(&status) {
            return Err(map_oauth_error(status, &raw, header_debug_id));
        }
        serde_json::from_slice(&raw).map_err(|error| {
            PaymentError::Serialization(format!("invalid paypal token response: {error}"))
        })
    }
}

fn log_failure(status: u16, code: Option<&str>, debug_id: Option<&str>) {
    tracing::warn!(
        provider = "paypal",
        http_status = status,
        code = code.unwrap_or_default(),
        debug_id = debug_id.unwrap_or_default(),
        "paypal request failed"
    );
}

fn truncate(message: &str, limit: usize) -> String {
    message.chars().take(limit).collect()
}

/// First `details[].issue` value (PayPal's machine-readable failure reason).
fn error_issue(value: &Value) -> Option<&str> {
    value
        .get("details")
        .and_then(Value::as_array)
        .and_then(|details| {
            details
                .iter()
                .find_map(|detail| detail.get("issue").and_then(Value::as_str))
        })
}

/// Map a non-2xx Orders/Payments API response to a [`PaymentError`].
pub(crate) fn map_api_error(
    status: u16,
    body: &[u8],
    header_debug_id: Option<String>,
) -> PaymentError {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let code = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut message = value
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("paypal request failed")
        .to_string();
    if let Some(issue) = error_issue(&value) {
        message = format!("{message} ({issue})");
    }
    let debug_id = value
        .get("debug_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(header_debug_id);
    log_failure(status, code.as_deref(), debug_id.as_deref());
    map_error_response(status, code, truncate(&message, 512), debug_id)
}

/// Map a non-2xx OAuth2 token response (`{"error", "error_description"}`).
pub(crate) fn map_oauth_error(
    status: u16,
    body: &[u8],
    header_debug_id: Option<String>,
) -> PaymentError {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let code = value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = value
        .get("error_description")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("paypal token request failed");
    log_failure(status, code.as_deref(), header_debug_id.as_deref());
    map_error_response(status, code, truncate(message, 512), header_debug_id)
}

fn map_error_response(
    status: u16,
    code: Option<String>,
    message: String,
    debug_id: Option<String>,
) -> PaymentError {
    match status {
        404 => PaymentError::not_found(PROVIDER, message),
        409 => PaymentError::conflict(PROVIDER, message),
        // PayPal reports semantic/business-rule failures as 422. Keep the
        // caller-visible detail bounded to the provider's sanitized message.
        422 => PaymentError::validation(message),
        // Authentication/authorization failures are configuration problems on
        // our side: `PaymentError::provider` maps 401/403 to a 502.
        _ => {
            let retryable = status == 429 || (500..=599).contains(&status);
            PaymentError::provider(PROVIDER, code, message, Some(status), debug_id, retryable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_not_found_conflict_and_validation() {
        let not_found = map_api_error(
            404,
            br#"{"name":"RESOURCE_NOT_FOUND","message":"The specified resource does not exist.","debug_id":"dbg-1"}"#,
            None,
        );
        assert!(matches!(not_found, PaymentError::NotFound { .. }));
        assert_eq!(not_found.http_status(), 404);
        assert_eq!(
            not_found.public_detail(),
            Some("The specified resource does not exist.")
        );

        let conflict = map_api_error(
            409,
            br#"{"name":"RESOURCE_CONFLICT","message":"A conflict occurred.","debug_id":"dbg-2"}"#,
            None,
        );
        assert!(matches!(conflict, PaymentError::Conflict { .. }));
        assert_eq!(conflict.http_status(), 409);

        let validation = map_api_error(
            422,
            br#"{"name":"UNPROCESSABLE_ENTITY","message":"Semantically incorrect.","debug_id":"dbg-3","details":[{"issue":"ORDER_ALREADY_CAPTURED"}]}"#,
            None,
        );
        assert!(matches!(validation, PaymentError::Validation { .. }));
        assert_eq!(validation.http_status(), 400);
        assert_eq!(
            validation.public_detail(),
            Some("Semantically incorrect. (ORDER_ALREADY_CAPTURED)")
        );
    }

    #[test]
    fn auth_failures_are_configuration_shaped_and_carry_the_debug_id() {
        let error = map_api_error(
            401,
            br#"{"name":"AUTHENTICATION_FAILURE","message":"Authentication failed.","debug_id":"dbg-4"}"#,
            Some("header-debug".to_string()),
        );
        assert_eq!(error.http_status(), 502);
        match error {
            PaymentError::Provider {
                code,
                request_id,
                retryable,
                ..
            } => {
                assert_eq!(code.as_deref(), Some("AUTHENTICATION_FAILURE"));
                // The body debug id wins over the header.
                assert_eq!(request_id.as_deref(), Some("dbg-4"));
                assert!(!retryable);
            }
            other => panic!("expected provider error, got {other:?}"),
        }
    }

    #[test]
    fn retryable_statuses_are_flagged() {
        for status in [429_u16, 500, 503] {
            let error = map_api_error(
                status,
                br#"{"name":"INTERNAL_SERVER_ERROR","message":"Ouch.","debug_id":"dbg-5"}"#,
                None,
            );
            assert!(error.retryable(), "status {status} should be retryable");
        }
        let bad_request = map_api_error(
            400,
            br#"{"name":"INVALID_REQUEST","message":"Bad.","debug_id":"dbg-6"}"#,
            None,
        );
        assert!(!bad_request.retryable());
        assert_eq!(bad_request.http_status(), 400);
    }

    #[test]
    fn non_json_error_bodies_fall_back_to_a_generic_message() {
        let error = map_api_error(503, b"<html>unavailable</html>", None);
        assert_eq!(error.http_status(), 502);
        assert!(error.retryable());
    }

    #[test]
    fn maps_oauth_errors() {
        let error = map_oauth_error(
            401,
            br#"{"error":"invalid_client","error_description":"Client authentication failed"}"#,
            None,
        );
        assert_eq!(error.http_status(), 502);
        match error {
            PaymentError::Provider { code, message, .. } => {
                assert_eq!(code.as_deref(), Some("invalid_client"));
                assert_eq!(message, "Client authentication failed");
            }
            other => panic!("expected provider error, got {other:?}"),
        }

        let unavailable = map_oauth_error(503, b"{}", None);
        assert!(unavailable.retryable());
    }

    #[test]
    fn long_messages_are_truncated() {
        let message = "x".repeat(900);
        let body = serde_json::json!({ "name": "SOMETHING", "message": message });
        let error = map_api_error(400, &serde_json::to_vec(&body).unwrap(), None);
        match error {
            PaymentError::Provider { message, .. } => assert_eq!(message.chars().count(), 512),
            other => panic!("expected provider error, got {other:?}"),
        }
    }
}
