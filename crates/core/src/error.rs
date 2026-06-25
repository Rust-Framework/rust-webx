//! Unified error type for the LRWF framework.

use thiserror::Error;

/// Framework-wide error type.
#[derive(Error, Debug)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("DI error: {0}")]
    Di(String),

    #[error("Routing error: {0}")]
    Routing(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

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
}

impl Error {
    /// Map the error to an appropriate HTTP status code.
    ///
    /// Used by the built-in exception middleware to produce
    /// well-formed HTTP error responses.
    pub fn status_code(&self) -> u16 {
        match self {
            Error::Http(_) => 400,
            Error::Di(_) => 500,
            Error::Routing(_) => 404,
            Error::Serialization(_) => 400,
            Error::Internal(_) => 500,
            Error::Message(_) => 500,
            Error::Validation(_) => 400,
            Error::NotFound(_) => 404,
        }
    }
}

/// Shorthand for Result<T, Error>.
pub type Result<T> = std::result::Result<T, Error>;
