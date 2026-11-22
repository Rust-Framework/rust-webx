//! Adyen Checkout JSON transport: headers, idempotency and error mapping.
//!
//! Every Checkout call is an authenticated JSON `POST` with `X-API-Key` and
//! `Content-Type: application/json`; mutating calls also send an
//! `Idempotency-Key`. Adyen returns structured errors
//! (`{status, errorCode, message, pspReference}`) which are mapped to
//! [`PaymentError`] so callers can decide retry/telemetry without parsing
//! provider text.

use serde_json::{json, Value};
use webx_client::{IHttpClient, OutboundRequest};
use webx_pay::{PaymentError, ProviderId};

const PROVIDER: ProviderId = ProviderId::Adyen;

/// Upper bound for one buffered Adyen response body (4 MiB).
pub(crate) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Send an authenticated JSON `POST` and return the parsed JSON body.
///
/// `path` is absolute (`"/v71/payments"`), `api_key` goes into `X-API-Key`, and
/// `idempotency_key` (when set) into `Idempotency-Key`. A non-2xx response is
/// converted into a typed [`PaymentError`]; it is never returned as a value.
pub(crate) async fn post_json(
    client: &dyn IHttpClient,
    api_key: &str,
    path: String,
    body: &Value,
    idempotency_key: Option<&str>,
) -> Result<Value, PaymentError> {
    let mut request = OutboundRequest::post(path)
        .header("x-api-key", api_key)
        .header("accept", "application/json");
    if let Some(key) = idempotency_key {
        request = request.header("idempotency-key", key);
    }
    let request = request.json(body).map_err(|error| {
        PaymentError::Serialization(format!("failed to encode adyen request: {error}"))
    })?;
    let response = client.execute(request).await?;
    let status = response.status;
    let body = response.body.collect(MAX_RESPONSE_BYTES).await?;
    let value: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).map_err(|error| {
            PaymentError::Serialization(format!("invalid adyen response: {error}"))
        })?
    };
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        let error_code = value
            .get("errorCode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        tracing::debug!(status, error_code, "adyen request failed");
        Err(map_error(status, &value))
    }
}

/// Map an Adyen error response to a [`PaymentError`].
///
/// * `422` - request validation failure (`PaymentError::Validation`, HTTP 400).
/// * `404` - unknown payment/refund (`PaymentError::NotFound`, HTTP 404).
/// * `401`/`403` - credential or permission problem on our side; exposed as a
///   provider error with status 401/403 so applications see HTTP 502 instead of
///   blaming the caller.
/// * `429` and `5xx` - retryable provider failures (HTTP 503).
/// * everything else - non-retryable provider failure.
pub(crate) fn map_error(status: u16, value: &Value) -> PaymentError {
    let error_code = value
        .get("errorCode")
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("adyen request failed")
        .to_string();
    let psp_reference = value
        .get("pspReference")
        .and_then(Value::as_str)
        .map(str::to_string);
    match status {
        422 => PaymentError::validation(match &error_code {
            Some(code) => format!("Adyen rejected the request ({code}): {message}"),
            None => format!("Adyen rejected the request: {message}"),
        }),
        404 => PaymentError::not_found(PROVIDER, message),
        // Authentication/permission failures are our configuration, not the
        // caller's; PaymentError maps them to HTTP 502 at the application border.
        401 | 403 => PaymentError::provider(
            PROVIDER,
            error_code,
            message,
            Some(status),
            psp_reference,
            false,
        ),
        429 | 500..=599 => PaymentError::provider(
            PROVIDER,
            error_code,
            message,
            Some(status),
            psp_reference,
            true,
        ),
        _ => PaymentError::provider(
            PROVIDER,
            error_code,
            message,
            Some(status),
            psp_reference,
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_errors_map_to_a_caller_error() {
        let error = map_error(
            422,
            &json!({
                "status": 422,
                "errorCode": "14_030",
                "message": "Return URL is missing.",
                "pspReference": "8816118280275544"
            }),
        );
        assert_eq!(error.http_status(), 400);
        assert!(error.public_detail().unwrap().contains("14_030"));
        assert!(error
            .public_detail()
            .unwrap()
            .contains("Return URL is missing."));
    }

    #[test]
    fn credential_failures_are_configuration_side_and_not_retried() {
        let error = map_error(
            401,
            &json!({
                "status": 401,
                "errorCode": "000",
                "message": "HTTP Status Response - Unauthorized"
            }),
        );
        assert_eq!(error.http_status(), 502);
        assert!(!error.retryable());
        assert!(error.public_detail().is_none());

        let error = map_error(
            403,
            &json!({"status": 403, "errorCode": "901", "message": "Invalid Merchant Account"}),
        );
        assert_eq!(error.http_status(), 502);
    }

    #[test]
    fn missing_payments_are_not_found() {
        let error = map_error(
            404,
            &json!({"status": 404, "errorCode": "905", "message": "Payment not found"}),
        );
        assert_eq!(error.http_status(), 404);
        assert_eq!(error.public_detail(), Some("Payment not found"));
    }

    #[test]
    fn rate_limits_and_server_errors_are_retryable() {
        for status in [429, 500, 502, 503] {
            let error = map_error(status, &json!({"message": "upstream unavailable"}));
            let expected_http = if status == 429 { 503 } else { 502 };
            assert_eq!(error.http_status(), expected_http, "status {status}");
            assert!(error.retryable(), "status {status}");
        }
    }

    #[test]
    fn other_client_errors_stay_typed_provider_errors() {
        let error = map_error(
            400,
            &json!({"status": 400, "errorCode": "702", "message": "Unexpected input"}),
        );
        assert_eq!(error.http_status(), 400);
        assert!(!error.retryable());
        assert!(error.public_detail().is_none());
    }
}
