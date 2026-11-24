//! Typed payment failures and their HTTP/`webx::Error` mapping.

use thiserror::Error;
use webx_client::HttpClientError;

use crate::ids::ProviderId;

/// A payment operation failure.
///
/// Errors carry enough structure for an application to decide retry/telemetry
/// without parsing provider text. `Display` is operator-facing: it may contain
/// provider codes, but never keys, certificates or full callback bodies.
#[derive(Debug, Error)]
pub enum PaymentError {
    /// Missing/invalid adapter configuration (startup validation).
    #[error("{provider} configuration error: {detail}")]
    Config {
        /// Provider whose configuration is invalid.
        provider: ProviderId,
        /// Field-level reason (never a secret value).
        detail: String,
    },

    /// The runtime configuration source (DB/KMS/config center) is unavailable.
    ///
    /// Retryable and mapped to 503: the caller should retry, and a webhook
    /// receiver should return 503 so the provider redelivers.
    #[error("{provider} configuration is unavailable: {detail}")]
    Unavailable {
        /// Provider whose configuration could not be resolved.
        provider: ProviderId,
        /// Operator-facing reason (never a secret value).
        detail: String,
    },

    /// Caller supplied an invalid request (amount, currency, channel, ...).
    #[error("invalid payment request: {detail}")]
    Validation {
        /// Human-readable reason.
        detail: String,
    },

    /// The provider adapter does not implement the requested capability.
    #[error("{provider} does not support {capability}")]
    Unsupported {
        /// Provider that lacks the capability.
        provider: ProviderId,
        /// Capability name (`capture`, `cancel`, `refund`, ...).
        capability: &'static str,
    },

    /// The provider rejected the operation.
    #[error("{provider} error: {message}")]
    Provider {
        /// Provider that returned the error.
        provider: ProviderId,
        /// Provider error code, when the API exposes one.
        code: Option<String>,
        /// Sanitized provider message.
        message: String,
        /// HTTP status returned by the provider.
        http_status: Option<u16>,
        /// Provider request id for support/debugging.
        request_id: Option<String>,
        /// Whether retrying the same request may succeed.
        retryable: bool,
    },

    /// A webhook signature did not verify.
    #[error("{provider} webhook signature is invalid: {reason}")]
    SignatureInvalid {
        /// Provider whose signature failed.
        provider: ProviderId,
        /// Fixed reason (`missing signature`, `timestamp outside tolerance`, ...).
        reason: &'static str,
    },

    /// The provider does not know the referenced payment/refund.
    #[error("{provider} resource not found: {message}")]
    NotFound {
        /// Provider queried.
        provider: ProviderId,
        /// Sanitized detail.
        message: String,
    },

    /// A state/idempotency conflict (same key, different request; already paid).
    #[error("{provider} conflict: {message}")]
    Conflict {
        /// Provider that reported the conflict.
        provider: ProviderId,
        /// Sanitized detail.
        message: String,
    },

    /// The outbound HTTP exchange failed (never a non-2xx response).
    #[error("payment transport error: {0}")]
    Transport(#[from] HttpClientError),

    /// A provider response could not be parsed or a request could not be encoded.
    #[error("payment serialization error: {0}")]
    Serialization(String),

    /// A local invariant failed (response verification, missing account, ...).
    #[error("payment internal error: {0}")]
    Internal(String),
}

impl PaymentError {
    /// Configuration error for `provider`.
    pub fn config(provider: ProviderId, detail: impl Into<String>) -> Self {
        Self::Config {
            provider,
            detail: detail.into(),
        }
    }

    /// Runtime configuration source unavailable (retryable 503).
    pub fn unavailable(provider: ProviderId, detail: impl Into<String>) -> Self {
        Self::Unavailable {
            provider,
            detail: detail.into(),
        }
    }

    /// Invalid request.
    pub fn validation(detail: impl Into<String>) -> Self {
        Self::Validation {
            detail: detail.into(),
        }
    }

    /// Capability not implemented by `provider`.
    pub fn unsupported(provider: ProviderId, capability: &'static str) -> Self {
        Self::Unsupported {
            provider,
            capability,
        }
    }

    /// Provider-reported failure.
    pub fn provider(
        provider: ProviderId,
        code: Option<String>,
        message: impl Into<String>,
        http_status: Option<u16>,
        request_id: Option<String>,
        retryable: bool,
    ) -> Self {
        Self::Provider {
            provider,
            code,
            message: message.into(),
            http_status,
            request_id,
            retryable,
        }
    }

    /// Signature failure with a fixed reason.
    pub fn signature(provider: ProviderId, reason: &'static str) -> Self {
        Self::SignatureInvalid { provider, reason }
    }

    /// Missing resource.
    pub fn not_found(provider: ProviderId, message: impl Into<String>) -> Self {
        Self::NotFound {
            provider,
            message: message.into(),
        }
    }

    /// State/idempotency conflict.
    pub fn conflict(provider: ProviderId, message: impl Into<String>) -> Self {
        Self::Conflict {
            provider,
            message: message.into(),
        }
    }

    /// Local invariant failure.
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal(detail.into())
    }

    /// Whether retrying the same request could succeed.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Transport(error) => error.is_connect(),
            Self::Provider { retryable, .. } => *retryable,
            Self::Unavailable { .. } => true,
            _ => false,
        }
    }

    /// HTTP status an application endpoint should return for this failure.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Validation { .. } | Self::Unsupported { .. } | Self::SignatureInvalid { .. } => {
                400
            }
            Self::NotFound { .. } => 404,
            Self::Conflict { .. } => 409,
            Self::Unavailable { .. } | Self::Transport(_) => 503,
            Self::Provider { http_status, .. } => match http_status {
                Some(400 | 422) => 400,
                Some(404) => 404,
                Some(409) => 409,
                Some(429) => 503,
                Some(500..=599) => 502,
                // Upstream auth/permission failures are configuration errors on
                // our side, not the caller's 401/403.
                Some(_) | None => 502,
            },
            Self::Config { .. } | Self::Serialization(_) | Self::Internal(_) => 500,
        }
    }

    /// Safe text for an HTTP problem detail (secrets and provider internals are
    /// never included).
    pub fn public_detail(&self) -> Option<&str> {
        match self {
            Self::Validation { detail } => Some(detail),
            Self::Unsupported { capability, .. } => Some(capability),
            Self::SignatureInvalid { reason, .. } => Some(reason),
            Self::NotFound { message, .. } | Self::Conflict { message, .. } => Some(message),
            Self::Provider { .. }
            | Self::Config { .. }
            | Self::Unavailable { .. }
            | Self::Transport(_)
            | Self::Serialization(_)
            | Self::Internal(_) => None,
        }
    }

    /// Provider involved, when the failure belongs to one.
    pub fn provider_id(&self) -> Option<&ProviderId> {
        match self {
            Self::Config { provider, .. }
            | Self::Unavailable { provider, .. }
            | Self::Unsupported { provider, .. }
            | Self::Provider { provider, .. }
            | Self::SignatureInvalid { provider, .. }
            | Self::NotFound { provider, .. }
            | Self::Conflict { provider, .. } => Some(provider),
            Self::Validation { .. }
            | Self::Transport(_)
            | Self::Serialization(_)
            | Self::Internal(_) => None,
        }
    }
}

/// Application-border mapping: `?` in a handler turns a payment failure into the
/// framework's problem+json response without losing the status class.
impl From<PaymentError> for webx_core::error::Error {
    fn from(error: PaymentError) -> Self {
        let status = error.http_status();
        let detail = error
            .public_detail()
            .map(str::to_string)
            .unwrap_or_else(|| "payment operation failed".to_string());
        match status {
            400 => webx_core::error::Error::validation(detail),
            404 => webx_core::error::Error::not_found(detail),
            409 => webx_core::error::Error::conflict(detail),
            503 => webx_core::error::Error::status(503, detail),
            _ => webx_core::error::Error::internal(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_status_mapping_is_fail_closed() {
        let auth =
            PaymentError::provider(ProviderId::Stripe, None, "bad key", Some(401), None, false);
        assert_eq!(auth.http_status(), 502);
        let limit = PaymentError::provider(
            ProviderId::Stripe,
            Some("rate_limit".into()),
            "slow down",
            Some(429),
            None,
            true,
        );
        assert_eq!(limit.http_status(), 503);
        assert!(limit.retryable());
        assert!(limit.public_detail().is_none());
    }

    #[test]
    fn signature_failure_is_a_client_error() {
        let error = PaymentError::signature(ProviderId::WechatPay, "missing signature");
        assert_eq!(error.http_status(), 400);
        assert_eq!(error.public_detail(), Some("missing signature"));
    }
}
