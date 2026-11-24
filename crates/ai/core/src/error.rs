//! OpenAI-compatible error envelope and host error writer.
//!
//! OpenAI SDKs expect failures as `{"error":{"message","type","param","code"}}`
//! rather than RFC 7807 `problem+json`. [`OpenAiErrorWriter`] is registered with
//! `HostBuilder::add_error_writer` and takes over for `/v1/` paths (including
//! 404s, where no route pattern exists); every other path keeps the framework's
//! `problem+json` response.

use async_trait::async_trait;
use serde::Serialize;
use webx_core::error::{Error, IErrorResponseWriter};
use webx_core::http::IHttpContext;

/// OpenAI-style error envelope.
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiError {
    /// The nested error object.
    pub error: OpenAiErrorBody,
}

/// Body of an OpenAI-style error.
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiErrorBody {
    /// Client-facing message (5xx messages are generic).
    pub message: String,
    /// OpenAI error type, e.g. `invalid_request_error`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Offending parameter, when known.
    pub param: Option<String>,
    /// Stable error code, e.g. `rate_limit_exceeded`.
    pub code: &'static str,
}

impl OpenAiError {
    /// Build an envelope from an HTTP status and message.
    pub fn from_status(status: u16, message: impl Into<String>) -> Self {
        let (kind, code) = classification(status);
        Self {
            error: OpenAiErrorBody {
                message: message.into(),
                kind,
                param: None,
                code,
            },
        }
    }

    /// The OpenAI `insufficient_quota` envelope (budget/dimension exhausted).
    pub fn quota(message: impl Into<String>) -> Self {
        Self {
            error: OpenAiErrorBody {
                message: message.into(),
                kind: "insufficient_quota",
                param: None,
                code: "insufficient_quota",
            },
        }
    }

    /// Build an envelope from a framework error, hiding 5xx detail.
    pub fn from_error(status: u16, error: &Error) -> Self {
        let message = if status >= 500 {
            "an unexpected error occurred".to_string()
        } else {
            error
                .public_detail()
                .unwrap_or("request failed")
                .to_string()
        };
        Self::from_status(status, message)
    }
}

/// Map a status to OpenAI's `type`/`code` pair.
fn classification(status: u16) -> (&'static str, &'static str) {
    match status {
        400 => ("invalid_request_error", "invalid_request"),
        401 => ("authentication_error", "invalid_api_key"),
        403 => ("permission_error", "permission_denied"),
        404 => ("not_found_error", "not_found"),
        409 => ("conflict_error", "conflict"),
        413 => ("request_too_large", "request_too_large"),
        415 => ("unsupported_media_type", "unsupported_media_type"),
        429 => ("rate_limit_error", "rate_limit_exceeded"),
        502 => ("server_error", "upstream_error"),
        503 => ("server_error", "service_unavailable"),
        504 => ("server_error", "upstream_timeout"),
        _ => ("server_error", "server_error"),
    }
}

/// Writes OpenAI-style errors for a route prefix.
pub struct OpenAiErrorWriter {
    path_prefix: String,
}

impl OpenAiErrorWriter {
    /// A writer for `/v1/`.
    pub fn new() -> Self {
        Self {
            path_prefix: "/v1/".to_string(),
        }
    }

    /// Use a different path prefix.
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = prefix.into();
        self
    }

    /// Whether `path` belongs to this surface.
    pub fn matches_path(&self, path: &str) -> bool {
        path.starts_with(&self.path_prefix)
    }
}

impl Default for OpenAiErrorWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IErrorResponseWriter for OpenAiErrorWriter {
    fn matches(&self, route_pattern: Option<&str>, path: &str, _accept: Option<&str>) -> bool {
        route_pattern
            .map(|pattern| pattern.starts_with(&self.path_prefix))
            .unwrap_or(false)
            || self.matches_path(path)
    }

    async fn write(&self, ctx: &mut dyn IHttpContext, error: &Error) {
        let status = error.status_code();
        let body = OpenAiError::from_error(status, error);
        let bytes = serde_json::to_vec(&body).unwrap_or_default();

        ctx.response_mut().set_status(status);
        ctx.response_mut()
            .set_header("content-type", "application/json");
        let _ = ctx.response_mut().write_bytes(bytes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_envelope_has_the_openai_shape() {
        let error = OpenAiError::from_status(429, "slow down");
        let value = serde_json::to_value(error).expect("serializes");
        assert_eq!(value["error"]["message"], "slow down");
        assert_eq!(value["error"]["type"], "rate_limit_error");
        assert_eq!(value["error"]["code"], "rate_limit_exceeded");
        assert!(value["error"]["param"].is_null());
    }

    #[test]
    fn five_hundreds_do_not_echo_internal_detail() {
        let error =
            OpenAiError::from_error(500, &Error::internal("db password in /var/lib/secret"));
        assert_eq!(error.error.message, "an unexpected error occurred");
        assert_eq!(error.error.kind, "server_error");
        assert!(!error.error.message.contains("secret"));
    }

    #[test]
    fn client_errors_keep_public_detail() {
        let error = OpenAiError::from_error(400, &Error::validation("model is required"));
        assert_eq!(error.error.message, "model is required");
        assert_eq!(error.error.kind, "invalid_request_error");
    }

    #[test]
    fn writer_matches_only_its_prefix() {
        let writer = OpenAiErrorWriter::new();
        assert!(writer.matches_path("/v1/chat/completions"));
        assert!(!writer.matches_path("/api/health"));
        assert!(writer.matches(Some("/v1/models"), "/v1/models", None));
        assert!(writer.matches(None, "/v1/missing", None));
        assert!(!writer.matches(Some("/api/users"), "/api/users", None));
    }

    #[test]
    fn classification_covers_gateway_statuses() {
        let cases = [
            (400, "invalid_request_error", "invalid_request"),
            (401, "authentication_error", "invalid_api_key"),
            (429, "rate_limit_error", "rate_limit_exceeded"),
            (502, "server_error", "upstream_error"),
            (504, "server_error", "upstream_timeout"),
        ];
        for (status, kind, code) in cases {
            let (actual_kind, actual_code) = classification(status);
            assert_eq!(actual_kind, kind);
            assert_eq!(actual_code, code);
        }
        assert_eq!(
            serde_json::to_value(OpenAiError::from_status(404, "x")).unwrap()["error"]["code"],
            json!("not_found")
        );
    }
}
