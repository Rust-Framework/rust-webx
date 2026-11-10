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

/// Shorthand for Result<T, Error>.
pub type Result<T> = std::result::Result<T, Error>;

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
}
