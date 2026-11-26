//! Framework error type.
//!
//! 2.9.1 uses the shared [`rust_core::AppError`] as the one application error
//! carrier; `webx::Error` is an alias, not a second definition. `EFError` and
//! `RdiError` remain typed domain diagnostics in their own crates and convert
//! at the boundary (`From` impls live there).
//!
//! Historical extension points keep their paths:
//! * [`Result`] / [`ActionResult`] are aliases of `rust_core::Result`;
//! * [`IErrorResponseWriter`] still lets a surface rewrite the wire envelope;
//! * [`default_code_for_status`] / [`status_is_retryable`] are re-exported for
//!   existing call sites.

use async_trait::async_trait;

pub use rust_core::error_codes::{default_code_for_status, status_is_retryable};
pub use rust_core::AppError as Error;

/// Shorthand for `Result<T, Error>` — the general fallible type used by
/// infrastructure code, helpers and middleware.
pub type Result<T> = rust_core::Result<T>;

/// The result of a request handler — the framework-level counterpart of
/// ASP.NET Core's `ActionResult<T>`.
///
/// Same underlying type as [`Result`]; the name is the handler-facing one:
/// `IRequestHandler::handle`, `IPipelineBehavior::handle` and `Next::run`
/// return it, so action code reads like the ASP.NET Core action it
/// corresponds to. `T` defaults to `()` for an action without a body.
pub type ActionResult<T = ()> = rust_core::Result<T>;

/// Rewrites framework-generated error responses for a surface.
///
/// The default host writes RFC 9457 `problem+json`. An API that must speak a
/// different envelope (for example the OpenAI `{"error":{...}}` shape under
/// `/v1/`) registers a writer; the first matching writer handles the error.
///
/// `matches` receives both the matched route pattern (if any) and the raw path,
/// so a writer can target a surface even for 404s where no route matched. It
/// must be cheap and side-effect free.
///
/// A writer that flattens [`Error::extensions`](rust_core::AppError::extensions)
/// into its body must use
/// [`extensions_for_wire`](crate::problem::extensions_for_wire): the
/// `x-webx-*` keys are internal carriers and must never reach the wire.
#[async_trait]
pub trait IErrorResponseWriter: Send + Sync {
    /// Whether this writer handles the error for this request.
    fn matches(&self, route_pattern: Option<&str>, path: &str, accept: Option<&str>) -> bool;

    /// Write the error response. The writer owns status, headers and body.
    async fn write(&self, ctx: &mut dyn crate::http::IHttpContext, error: &Error);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serde_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{").unwrap_err()
    }

    #[test]
    fn request_serialization_stays_a_client_error() {
        let error: Error = serde_error().into();
        assert_eq!(error.status_code(), 400);
        assert_eq!(error.code(), "invalid_json");
        assert_eq!(error.public_detail(), None);
    }

    #[test]
    fn di_failures_convert_to_a_hidden_di_error() {
        let error: Error = rust_dix::RdiError::not_found("OrderService", None).into();
        assert_eq!(error.status_code(), 500);
        assert_eq!(error.code(), "dependency_injection");
        assert_eq!(error.public_detail(), None);
        assert!(error
            .operator()
            .is_some_and(|detail| detail.contains("OrderService")));
    }

    #[test]
    fn codes_and_retryability_are_stable() {
        assert_eq!(Error::validation("x").code(), "validation_failed");
        assert_eq!(Error::not_found("x").code(), "not_found");
        assert_eq!(Error::unauthorized("x").code(), "unauthorized");
        assert_eq!(Error::status(503, "down").code(), "service_unavailable");
        assert_eq!(Error::internal("x").code(), "internal_error");

        assert!(Error::status(503, "down").retryable());
        assert!(Error::status(429, "slow down").retryable());
        assert!(!Error::status(500, "boom").retryable());
        assert!(!Error::internal("boom").retryable());
        assert!(!Error::validation("bad").retryable());
    }

    #[test]
    fn typed_rate_limit_carries_retry_after() {
        let limited = Error::too_many_requests(7, "quota exhausted");
        assert_eq!(limited.status_code(), 429);
        assert_eq!(limited.code(), "too_many_requests");
        assert!(limited.retryable());
        assert_eq!(limited.public_detail(), Some("quota exhausted"));
        assert_eq!(crate::problem::retry_after_of(&limited), Some(7));
        assert_eq!(
            crate::problem::retry_after_of(&Error::status(429, "slow")),
            None
        );
    }
}
