//! Outbound HTTP transport: request signing, response signature verification
//! and WeChat Pay error mapping.
//!
//! A single `IHttpClient` instance per adapter carries every API call; the
//! adapter signs the exact bytes it sends (JSON is serialized once, then both
//! signed and transmitted) so the server-side signature cannot be invalidated by
//! a second serialization.
//!
//! Response policy:
//!
//! * A **complete** set of `Wechatpay-*` signature headers must verify for every
//!   2xx response; missing headers are a hard failure and the body is discarded.
//! * Error responses (`>= 400`) are verified when the signature headers are
//!   present. If a proxy/CDN stripped the headers the body is still parsed, but
//!   it can only produce an error (never payment state), so verification adds no
//!   trust opportunity there.
//! * `GET /v3/certificates` is the bootstrap trust anchor and, per the official
//!   docs, its response is not signature-verified; every subsequent response is
//!   verified with the downloaded platform keys.

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use rsa::PublicKey;
use serde_json::Value;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_pay::PaymentError;

use crate::cert::{normalize_serial, parse_certificates_response, CertificateCache};
use crate::mapping::string_field;
use crate::provider::{AccountKeyMode, WechatPayAccount};
use crate::sign::{
    authorization_header, new_nonce, request_signature_message, response_signature_message,
    sign_message_base64, validate_timestamp, verify_message_base64,
};
use crate::time::unix_now;
use crate::webhook::SIGNATURE_TOLERANCE_SECONDS;
use crate::{MAX_BODY_BYTES, PROVIDER};

const CERTIFICATES_PATH: &str = "/v3/certificates";

#[derive(Clone, Copy)]
pub(crate) enum Verb {
    Get,
    Post,
}

impl Verb {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// A parsed JSON response plus the provider request id.
pub(crate) struct JsonResponse {
    pub(crate) value: Value,
    pub(crate) request_id: Option<String>,
}

/// Raw HTTP response, before signature verification / error mapping.
struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Bytes,
    request_id: Option<String>,
}

/// The four documented response/notification signature headers.
struct ResponseSignature {
    timestamp: String,
    nonce: String,
    signature: String,
    serial: String,
}

impl ResponseSignature {
    fn from_headers(headers: &[(String, String)]) -> Option<Self> {
        Some(Self {
            timestamp: header(headers, "wechatpay-timestamp")?,
            nonce: header(headers, "wechatpay-nonce")?,
            signature: header(headers, "wechatpay-signature")?,
            serial: header(headers, "wechatpay-serial")?,
        })
    }
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// HTTP transport dedicated to one [`WechatPayOptions`](crate::WechatPayOptions)
/// configuration.
pub(crate) struct WechatPayTransport {
    injected_client: Option<Arc<dyn IHttpClient>>,
    client: OnceLock<Result<Arc<dyn IHttpClient>, String>>,
    options: HttpClientOptions,
}

impl WechatPayTransport {
    pub(crate) fn new(options: HttpClientOptions) -> Self {
        Self {
            injected_client: None,
            client: OnceLock::new(),
            options,
        }
    }

    pub(crate) fn with_client(options: HttpClientOptions, client: Arc<dyn IHttpClient>) -> Self {
        let mut transport = Self::new(options);
        transport.injected_client = Some(client);
        transport
    }

    /// Build (or reuse) the pooled HTTP client. Never performs a request.
    pub(crate) fn client(&self) -> Result<Arc<dyn IHttpClient>, String> {
        if let Some(client) = &self.injected_client {
            return Ok(Arc::clone(client));
        }
        let built = self.client.get_or_init(|| {
            HttpClient::new("wechatpay", self.options.clone())
                .map(|client| Arc::new(client) as Arc<dyn IHttpClient>)
                .map_err(|error| error.to_string())
        });
        built.as_ref().map(Arc::clone).map_err(Clone::clone)
    }

    pub(crate) async fn get_json(
        &self,
        account: &WechatPayAccount,
        path_and_query: &str,
    ) -> Result<JsonResponse, PaymentError> {
        self.send(account, Verb::Get, path_and_query, None).await
    }

    pub(crate) async fn post_json(
        &self,
        account: &WechatPayAccount,
        path_and_query: &str,
        value: &Value,
    ) -> Result<JsonResponse, PaymentError> {
        let body = serde_json::to_vec(value).map_err(|error| {
            PaymentError::Serialization(format!("cannot encode WeChat Pay request body: {error}"))
        })?;
        self.send(account, Verb::Post, path_and_query, Some(body))
            .await
    }

    async fn send(
        &self,
        account: &WechatPayAccount,
        verb: Verb,
        path_and_query: &str,
        body: Option<Vec<u8>>,
    ) -> Result<JsonResponse, PaymentError> {
        let response = self.send_raw(account, verb, path_and_query, body).await?;
        let success = (200..300).contains(&response.status);
        if let Some(signature) = ResponseSignature::from_headers(&response.headers) {
            self.verify_response(account, &response.body, signature)
                .await?;
        } else if success {
            return Err(PaymentError::Internal(format!(
                "wechatpay response ({}) has no Wechatpay-Signature headers; refusing unverified data",
                response.status
            )));
        }
        if !success {
            return Err(map_error_response(response));
        }
        let value = if response.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&response.body).map_err(|error| {
                PaymentError::Serialization(format!("invalid WeChat Pay JSON response: {error}"))
            })?
        };
        Ok(JsonResponse {
            value,
            request_id: response.request_id,
        })
    }

    async fn send_raw(
        &self,
        account: &WechatPayAccount,
        verb: Verb,
        path_and_query: &str,
        body: Option<Vec<u8>>,
    ) -> Result<RawResponse, PaymentError> {
        let client = self
            .client()
            .map_err(|error| PaymentError::config(PROVIDER, error))?;
        let timestamp = unix_now();
        let nonce = new_nonce();
        let body_bytes: &[u8] = body.as_deref().unwrap_or(&[]);
        let message =
            request_signature_message(verb.as_str(), path_and_query, timestamp, &nonce, body_bytes);
        let signature = sign_message_base64(&account.signing_key, &message).map_err(|error| {
            PaymentError::Internal(format!("wechatpay request signing failed: {error}"))
        })?;
        let authorization = authorization_header(
            &account.mch_id,
            &account.serial_no,
            timestamp,
            &nonce,
            &signature,
        );
        let mut request = match verb {
            Verb::Get => OutboundRequest::get(path_and_query),
            Verb::Post => OutboundRequest::post(path_and_query),
        }
        .header("authorization", authorization)
        .header("accept", "application/json");
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(Bytes::from(body));
        }
        let response = client.execute(request).await?;
        let status = response.status;
        let request_id = response
            .headers
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let headers: Vec<(String, String)> = response
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect();
        let body = response.body.collect(MAX_BODY_BYTES).await?;
        Ok(RawResponse {
            status,
            headers,
            body,
            request_id,
        })
    }

    async fn verify_response(
        &self,
        account: &WechatPayAccount,
        body: &[u8],
        signature: ResponseSignature,
    ) -> Result<(), PaymentError> {
        let now = unix_now();
        validate_timestamp(&signature.timestamp, now, SIGNATURE_TOLERANCE_SECONDS).map_err(
            |reason| {
                PaymentError::Internal(format!(
                    "wechatpay response signature timestamp is invalid: {reason}"
                ))
            },
        )?;
        let key = self.verification_key(account, &signature.serial).await?;
        let message = response_signature_message(&signature.timestamp, &signature.nonce, body);
        verify_message_base64(&key, &message, &signature.signature).map_err(|reason| {
            PaymentError::Internal(format!(
                "wechatpay response signature verification failed: {reason}"
            ))
        })
    }

    /// Resolve the platform key named by a `Wechatpay-Serial` header.
    ///
    /// Public-key mode requires the header to equal the configured
    /// `PUB_KEY_ID_...`. Platform-certificate mode refreshes the certificate list
    /// at most once per [`CERTIFICATE_REFRESH_INTERVAL`](crate::CERTIFICATE_REFRESH_INTERVAL)
    /// and fails closed when the serial is still unknown.
    pub(crate) async fn verification_key(
        &self,
        account: &WechatPayAccount,
        serial: &str,
    ) -> Result<Arc<PublicKey>, PaymentError> {
        match &account.key_mode {
            AccountKeyMode::PublicKey {
                public_key_id,
                public_key,
            } => {
                if !serial.trim().is_empty()
                    && normalize_serial(serial) == normalize_serial(public_key_id)
                {
                    Ok(Arc::clone(public_key))
                } else {
                    Err(PaymentError::Internal(format!(
                        "wechatpay response serial `{serial}` does not match the configured public key id `{public_key_id}`"
                    )))
                }
            }
            AccountKeyMode::PlatformCertificates(cache) => {
                self.platform_key(account, cache, serial).await
            }
        }
    }

    async fn platform_key(
        &self,
        account: &WechatPayAccount,
        cache: &Arc<CertificateCache>,
        serial: &str,
    ) -> Result<Arc<PublicKey>, PaymentError> {
        if let Some(key) = cache.get(serial) {
            return Ok(key);
        }
        if cache.try_begin_refresh() {
            self.refresh_certificates(account).await?;
        }
        cache.get(serial).ok_or_else(|| {
            PaymentError::Internal(format!(
                "wechatpay platform certificate `{serial}` is unknown; certificate refresh was rate-limited or the serial is not published"
            ))
        })
    }

    /// Download and cache the platform certificates (`GET /v3/certificates`).
    pub(crate) async fn refresh_certificates(
        &self,
        account: &WechatPayAccount,
    ) -> Result<(), PaymentError> {
        let cache = match &account.key_mode {
            AccountKeyMode::PlatformCertificates(cache) => cache,
            AccountKeyMode::PublicKey { .. } => return Ok(()),
        };
        let response = self
            .send_raw(account, Verb::Get, CERTIFICATES_PATH, None)
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(map_error_response(response));
        }
        // Official exemption: the platform-certificate download is the bootstrap
        // trust anchor (see the module docs and the crate docs).
        let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
            PaymentError::Serialization(format!(
                "invalid WeChat Pay certificates response: {error}"
            ))
        })?;
        let certificates =
            parse_certificates_response(&account.api_v3_key, &value).map_err(|error| {
                PaymentError::Serialization(format!("wechatpay platform certificates: {error}"))
            })?;
        for (serial, key) in certificates {
            cache.insert(&serial, key);
        }
        tracing::debug!(
            account = %account.id,
            count = cache.len(),
            "wechatpay platform certificates refreshed"
        );
        Ok(())
    }
}

/// Map a non-2xx WeChat Pay response to [`PaymentError`].
///
/// Documented classes: 401 is a merchant credential/signature configuration
/// problem, 403 splits into permission configuration (`NO_AUTH`) and business
/// rule errors, 404 is a missing order/refund, 429 and 5xx are retryable.
/// Duplicate-order errors are idempotency conflicts. The provider message is
/// kept for operators but never becomes a public HTTP detail.
fn map_error_response(response: RawResponse) -> PaymentError {
    let status = response.status;
    let (code, message) = parse_error_body(&response.body);
    let request_id = response.request_id;
    let code_ref = code.as_deref();
    if status == 404 {
        return PaymentError::not_found(PROVIDER, message);
    }
    if matches!(code_ref, Some("ORDER_CLOSED" | "OUT_TRADE_NO_USED")) {
        return PaymentError::conflict(PROVIDER, message);
    }
    if status == 401 || (status == 403 && code_ref == Some("NO_AUTH")) {
        return PaymentError::config(
            PROVIDER,
            format!(
                "WeChat Pay rejected the merchant credentials or permissions (http {status}, code {})",
                code_ref.unwrap_or("unknown")
            ),
        );
    }
    let retryable = status == 429 || status >= 500;
    PaymentError::provider(PROVIDER, code, message, Some(status), request_id, retryable)
}

fn parse_error_body(body: &[u8]) -> (Option<String>, String) {
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => {
            let code = string_field(&value, "code").map(str::to_string);
            let message = string_field(&value, "message")
                .unwrap_or("WeChat Pay request failed")
                .to_string();
            (code, message)
        }
        Err(_) => (None, "WeChat Pay request failed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(status: u16, value: Value) -> RawResponse {
        RawResponse {
            status,
            headers: Vec::new(),
            body: Bytes::from(serde_json::to_vec(&value).unwrap()),
            request_id: Some("req-1".to_string()),
        }
    }

    #[test]
    fn maps_not_found_and_conflicts_to_typed_errors() {
        let error = map_error_response(raw(
            404,
            json!({ "code": "ORDER_NOT_EXIST", "message": "order not found" }),
        ));
        assert!(matches!(error, PaymentError::NotFound { .. }));
        assert_eq!(error.http_status(), 404);

        let error = map_error_response(raw(
            403,
            json!({ "code": "OUT_TRADE_NO_USED", "message": "duplicate order" }),
        ));
        assert!(matches!(error, PaymentError::Conflict { .. }));
        assert_eq!(error.http_status(), 409);
    }

    #[test]
    fn maps_credential_failures_to_configuration_errors() {
        let error = map_error_response(raw(
            401,
            json!({ "code": "SIGN_ERROR", "message": "签名错误" }),
        ));
        assert!(matches!(error, PaymentError::Config { .. }));
        assert_eq!(error.http_status(), 500);

        let error = map_error_response(raw(
            403,
            json!({ "code": "NO_AUTH", "message": "no permission" }),
        ));
        assert!(matches!(error, PaymentError::Config { .. }));
    }

    #[test]
    fn maps_retryable_and_business_failures() {
        let error = map_error_response(raw(
            429,
            json!({ "code": "FREQUENCY_LIMITED", "message": "slow down" }),
        ));
        assert!(matches!(error, PaymentError::Provider { .. }));
        assert!(error.retryable());
        assert_eq!(error.http_status(), 503);

        let error = map_error_response(raw(
            500,
            json!({ "code": "SYSTEM_ERROR", "message": "system" }),
        ));
        assert!(error.retryable());

        let error = map_error_response(raw(
            403,
            json!({ "code": "NOT_ENOUGH", "message": "balance" }),
        ));
        assert!(!error.retryable());
        assert_eq!(error.http_status(), 502);
    }

    #[test]
    fn non_json_error_bodies_still_map_to_a_provider_error() {
        let response = RawResponse {
            status: 502,
            headers: Vec::new(),
            body: Bytes::from_static(b"<html>bad gateway</html>"),
            request_id: None,
        };
        let error = map_error_response(response);
        assert!(matches!(error, PaymentError::Provider { .. }));
        assert!(error.retryable());
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let headers = vec![
            ("Wechatpay-Timestamp".to_string(), "1700000000".to_string()),
            ("wechatpay-nonce".to_string(), "nonce".to_string()),
            ("WECHATPAY-SIGNATURE".to_string(), "sig".to_string()),
            ("wechatpay-serial".to_string(), "SERIAL".to_string()),
        ];
        let signature = ResponseSignature::from_headers(&headers).unwrap();
        assert_eq!(signature.timestamp, "1700000000");
        assert_eq!(signature.nonce, "nonce");
        assert_eq!(signature.signature, "sig");
        assert_eq!(signature.serial, "SERIAL");
        assert!(ResponseSignature::from_headers(&headers[..3]).is_none());
    }

    #[test]
    fn transport_builds_the_pooled_client_without_network_access() {
        let transport = WechatPayTransport::new(crate::test_support::options().http_options());
        let client = transport.client().expect("client construction");
        assert_eq!(client.name(), "wechatpay");
    }
}
