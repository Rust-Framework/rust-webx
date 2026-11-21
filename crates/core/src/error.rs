//! Unified error type for the rust-webx framework.

use thiserror::Error;

/// Framework-wide error type.
#[derive(Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Status {0}: {1}")]
    Status(u16, String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("DI error: {0}")]
    Di(String),

    #[error("Routing error: {0}")]
    Routing(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Serializing the **response** failed — a server-side defect, not a bad
    /// request.
    ///
    /// Kept separate from [`Error::Serialization`] (which stays on the request
    /// parsing path and maps to 400) so that a DTO which cannot be serialized
    /// is reported as 500 instead of blaming the client. There is no `#[from]`
    /// because that conversion is already taken by `Serialization`; call sites
    /// convert explicitly with `.map_err(Error::ResponseSerialization)`.
    #[error("response serialization failed: {0}")]
    ResponseSerialization(#[source] serde_json::Error),

    /// A server-side state-machine / programming error, e.g. the response body
    /// state does not allow the requested operation. Maps to 500.
    #[error("{0}")]
    State(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    Message(String),

    /// Validation error with user-readable message.
    #[error("{0}")]
    Validation(String),

    /// Resource not found.
    #[error("{0}")]
    NotFound(String),

    /// Optimistic concurrency or state conflict.
    #[error("{0}")]
    Conflict(String),

    /// Request body (or one uploaded file) exceeded the configured limit.
    #[error("{0}")]
    PayloadTooLarge(String),

    /// Request content type is not supported by the target endpoint.
    #[error("{0}")]
    UnsupportedMediaType(String),

    /// A fully-formed problem response (validation, binding failure).
    ///
    /// Carried as-is instead of being flattened into a message so the host can
    /// write the status and the extension members (`errors`) verbatim.
    #[error("request problem details")]
    Problem(Box<crate::problem::ProblemDetails>),
}

impl Error {
    /// Map the error to an appropriate HTTP status code.
    ///
    /// Used by the built-in exception middleware to produce
    /// well-formed HTTP error responses.
    pub fn status_code(&self) -> u16 {
        match self {
            Error::Http(_) => 400,
            Error::Status(code, _) => *code,
            Error::Unauthorized(_) => 401,
            Error::Forbidden(_) => 403,
            Error::Di(_) => 500,
            Error::Routing(_) => 404,
            Error::Serialization(_) => 400,
            Error::ResponseSerialization(_) => 500,
            Error::State(_) => 500,
            Error::Internal(_) => 500,
            Error::Message(_) => 500,
            Error::Validation(_) => 400,
            Error::NotFound(_) => 404,
            Error::Conflict(_) => 409,
            Error::PayloadTooLarge(_) => 413,
            Error::UnsupportedMediaType(_) => 415,
            Error::Problem(problem) => problem.status,
        }
    }

    /// Stable machine-readable error code for `problem+json` clients.
    ///
    /// Codes are part of the wire contract: they let a client branch without
    /// parsing the human-readable `detail` (and without depending on a status
    /// code that may map to several causes). `Problem` errors keep the code the
    /// endpoint supplied.
    pub fn code(&self) -> std::borrow::Cow<'static, str> {
        use std::borrow::Cow;
        match self {
            Error::Http(_) => Cow::Borrowed("bad_request"),
            Error::Status(code, _) => Cow::Borrowed(default_code_for_status(*code)),
            Error::Unauthorized(_) => Cow::Borrowed("unauthorized"),
            Error::Forbidden(_) => Cow::Borrowed("forbidden"),
            Error::Di(_) => Cow::Borrowed("dependency_injection"),
            Error::Routing(_) => Cow::Borrowed("route_not_found"),
            Error::Serialization(_) => Cow::Borrowed("invalid_json"),
            Error::ResponseSerialization(_) => Cow::Borrowed("response_serialization"),
            Error::State(_) => Cow::Borrowed("invalid_state"),
            Error::Internal(_) | Error::Message(_) => Cow::Borrowed("internal_error"),
            Error::Validation(_) => Cow::Borrowed("validation_failed"),
            Error::NotFound(_) => Cow::Borrowed("not_found"),
            Error::Conflict(_) => Cow::Borrowed("conflict"),
            Error::PayloadTooLarge(_) => Cow::Borrowed("payload_too_large"),
            Error::UnsupportedMediaType(_) => Cow::Borrowed("unsupported_media_type"),
            Error::Problem(problem) => match problem.code() {
                Some(code) if !code.is_empty() => Cow::Owned(code.to_string()),
                _ => Cow::Borrowed(default_code_for_status(problem.status)),
            },
        }
    }

    /// Whether retrying the same request could succeed.
    ///
    /// Only transient conditions are retryable: rate limiting and upstream
    /// unavailability/timeout. A validation error or a server-side defect is
    /// not — retrying would just repeat the failure.
    pub fn retryable(&self) -> bool {
        match self {
            Error::Status(code, _) => status_is_retryable(*code),
            Error::Problem(problem) => problem
                .retryable()
                .unwrap_or_else(|| status_is_retryable(problem.status)),
            _ => false,
        }
    }

    /// The client-facing detail for a 4xx response.
    ///
    /// `Display` is written for operators (`"Unauthorized: …"`,
    /// `"Internal error: …"`), so serving it verbatim would leak variant names
    /// and internals. This returns the variant's own message for the errors
    /// that are safe to show, and `None` for 5xx/internal variants — the host
    /// then serves a stable detail and logs the original text.
    pub fn public_detail(&self) -> Option<&str> {
        match self {
            Error::Http(detail)
            | Error::Unauthorized(detail)
            | Error::Forbidden(detail)
            | Error::Routing(detail)
            | Error::Validation(detail)
            | Error::NotFound(detail)
            | Error::Conflict(detail)
            | Error::PayloadTooLarge(detail)
            | Error::UnsupportedMediaType(detail) => Some(detail),
            Error::Status(_, detail) => Some(detail),
            // Internal / DI / state / response serialization / already-written
            // problem: nothing here is safe or meaningful for a client.
            Error::Di(_)
            | Error::Serialization(_)
            | Error::ResponseSerialization(_)
            | Error::State(_)
            | Error::Internal(_)
            | Error::Message(_)
            | Error::Problem(_) => None,
        }
    }
}

/// Stable machine-readable code for a bare HTTP status.
///
/// Used when the error carries no more specific information (a raw status or a
/// problem built from a status).
pub fn default_code_for_status(status: u16) -> &'static str {
    match status {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        405 => "method_not_allowed",
        409 => "conflict",
        413 => "payload_too_large",
        415 => "unsupported_media_type",
        422 => "unprocessable_entity",
        429 => "too_many_requests",
        501 => "not_implemented",
        500 => "internal_error",
        502 => "bad_gateway",
        503 => "service_unavailable",
        504 => "gateway_timeout",
        _ => "error",
    }
}

/// Whether a bare HTTP status describes a transient condition worth retrying.
pub fn status_is_retryable(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

/// Shorthand for `Result<T, Error>` — the general fallible type used by
/// infrastructure code, helpers and middleware.
pub type Result<T> = std::result::Result<T, Error>;

/// The result of a request handler — the framework-level counterpart of
/// ASP.NET Core's `ActionResult<T>`.
///
/// Same underlying type as [`Result`]; the name is the handler-facing one:
/// `IRequestHandler::handle`, `IPipelineBehavior::handle` and `Next::run`
/// return it, so action code reads like the ASP.NET Core action it
/// corresponds to. `T` defaults to `()` for an action without a body.
pub type ActionResult<T = ()> = std::result::Result<T, Error>;

/// Rewrites framework-generated error responses for a surface.
///
/// The default host writes RFC 7807 `problem+json`. An API that must speak a
/// different envelope (for example the OpenAI `{"error":{...}}` shape under
/// `/v1/`) registers a writer; the first matching writer handles the error.
///
/// `matches` receives both the matched route pattern (if any) and the raw path,
/// so a writer can target a surface even for 404s where no route matched. It
/// must be cheap and side-effect free.
#[async_trait::async_trait]
pub trait IErrorResponseWriter: Send + Sync {
    /// Whether this writer handles the error for this request.
    fn matches(&self, route_pattern: Option<&str>, path: &str, accept: Option<&str>) -> bool;

    /// Write the error response. The writer owns status, headers and body.
    async fn write(&self, ctx: &mut dyn crate::http::IHttpContext, error: &Error);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any `serde_json::Error`; direction does not matter for the mapping test.
    fn serde_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{").unwrap_err()
    }

    #[test]
    fn response_serialization_maps_to_500_and_hides_detail() {
        let err = Error::ResponseSerialization(serde_error());
        assert_eq!(err.status_code(), 500);
        assert_eq!(err.public_detail(), None);
        assert!(err
            .to_string()
            .starts_with("response serialization failed:"));
    }

    #[test]
    fn state_error_maps_to_500_and_hides_detail() {
        let err = Error::State("response body already written".into());
        assert_eq!(err.status_code(), 500);
        assert_eq!(err.public_detail(), None);
        assert_eq!(err.to_string(), "response body already written");
    }

    #[test]
    fn request_serialization_stays_a_client_error() {
        // Request parsing keeps its `#[from]` conversion and its 400 mapping;
        // the new response-side variant must not change that.
        let err: Error = serde_error().into();
        assert!(matches!(err, Error::Serialization(_)));
        assert_eq!(err.status_code(), 400);
        assert_eq!(err.public_detail(), None);
    }

    #[test]
    fn codes_and_retryability_are_stable() {
        assert_eq!(Error::Validation("x".into()).code(), "validation_failed");
        assert_eq!(Error::NotFound("x".into()).code(), "not_found");
        assert_eq!(Error::Unauthorized("x".into()).code(), "unauthorized");
        assert_eq!(
            Error::Status(503, "down".into()).code(),
            "service_unavailable"
        );
        assert_eq!(Error::Internal("x".into()).code(), "internal_error");

        assert!(Error::Status(503, "down".into()).retryable());
        assert!(Error::Status(429, "slow down".into()).retryable());
        assert!(!Error::Status(500, "boom".into()).retryable());
        assert!(!Error::Internal("boom".into()).retryable());
        assert!(!Error::Validation("bad".into()).retryable());
    }

    #[test]
    fn problem_errors_keep_their_code_and_retryability() {
        let problem = crate::problem::ProblemDetails::new(429, "Too Many Requests")
            .with_code("rate_limited")
            .with_retryable(true);
        let error = problem.to_error();
        assert_eq!(error.status_code(), 429);
        assert_eq!(error.code(), "rate_limited");
        assert!(error.retryable());
    }
}
